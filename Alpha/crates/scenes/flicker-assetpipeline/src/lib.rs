//! flicker-assetpipeline — **Clayworks Bench**, the asset-pipeline editor: the interactive host
//! for the `flicker-content` stages, and the producer the Quartermaster's staging tier reviews.
//!
//! The realized workflow (golden spec 4916D78B, node A3A3259C): open a folder of raw
//! sources → a step wizard classifies what is in it → matches it to the canonical
//! 66-bone skeleton → bakes the one self-describing `flicker.rig` → hot-reloads in-app.
//! Design of record: DesignSync "Asset Processing Pipeline UI" (project
//! `2fc44682-9c08-41a6-bb9f-c415471b15e9`, `Asset Pipeline.dc.html`).
//!
//! **This crate hosts; it does not process.** Every stage is `flicker-content`'s
//! (`scan_folder` → `parse_fbx` → `rename_to_canonical` → `conform_to_canonical` →
//! `bake_rig`), the viewport is `flicker-render`'s shared [`QuadGrid`], the rig overlay
//! geometry is `flicker-mechanics::debug`, and the HUD is the `flicker-widgets`
//! component walker. Adding processing logic *here* would fork a pipeline that already
//! exists — the editor's job is to drive it and show its reports.
//!
//! **Slice status.** Load and Analyze run for real. Classify, Conform, Attach and Review
//! are navigable and render the state that genuinely exists (bone/vert/texture counts,
//! the conform reports) — they do NOT display invented numbers; a stage that is not wired
//! yet says so rather than showing a plausible figure.
//!
//! # The UI is DATA
//!
//! On the canonical pattern (rule E5AFBBAB — the Quartermaster is the reference), not the
//! shape this bench itself used to model: the scene owns no HUD Lua and composes nothing.
//! `ui_templates.json` holds the `clayworks_bench` proto — the whole surface over the
//! shared `workflow` spine, with `clayworks_bank` / `clayworks_row` / `clayworks_offset` /
//! `clayworks_import_card` beneath it. [`AssetPipeline::build_tree`] emits ONE instance
//! node, declares the screen's input as `on_<signal>` props, and calls `expand` at the
//! end — the single seam, so the scene and every gate walk the same tree. The script host
//! is here purely as the COMPONENT LIBRARY.
//!
//! **Controller is the floor.** A/B are the wizard's forward/back; the bumpers walk the
//! same rail, because the step strip IS this screen's tab bar; L2/R2 cycle the gizmo mode
//! on the rig page. Every declared intent has a dispatcher arm — a bound signal with no
//! arm is dead hardware, and `every_declared_intent_reaches_the_dispatcher` asserts it.
//!
//! # Its output is STAGED, not shipped
//!
//! Export writes into `content/staging/` via `flicker_content::roots`, never straight
//! into the package the game reads. "I imported an asset" and "the asset ships" are two
//! events now; the Quartermaster promotes the second.

use std::borrow::Cow;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use flicker_input_core::{
    AbstractControls, ContextualBindings, GamepadConfig, InputMap, InputState, Key,
};
use flicker::render::{
    build_textured_verts, grid_segments_xy, Camera, Mat4, MeshDrawOptions, MeshHandle, MeshIndices,
    MeshVertex, PbrMaps, QuadGrid, Rect, Renderer, SceneLighting, TextureHandle, TexturedMeshHandle,
    Vec2, Vec3,
};
use flicker::scene::{Scene, Transition};
use flicker::script::{ComponentLibrary, HudCommand, ScriptHost, UiNode, Value, ValueMap};
use flicker::ui::{
    builtin_templates, expand, load_styles, render_hud, run_ui_with, strings, workflows_from_json,
    TemplateRegistry, UiInput, UiIntents, UiState, WalkerHandler, Workflow, WorkflowDef,
    UI_COMPONENT_MODULES,
};
// The HUD renders through `run_ui_with` (Lua component dispatch); `run_ui` is test-only.
#[cfg(test)]
use flicker::ui::run_ui;
use flicker_shell::{PauseScene, Theme};

use flicker_content::{
    attach_world, classify_asset, conform_to_canonical, default_reference,
    fitting_base, garment_socket, parse_fbx, rename_to_canonical, scan_folder,
    bake_skin, source_maps, write_garment, write_prop, write_rig, AssetClass, AssetReport,
    ConformOutput, Fit,
    Kind, PropKind, RawModel, RenameReport, Scan, SourceMaps,
};
use flicker_mechanics::{
    autofit_capsules_from, closest_point_ray_segment, debug, drag_plane, gizmo_segments, GizmoMode,
    Shape, Volume,
};

mod route;
use route::RootHandler;

// The shared input event bus (spec section 9 port): the core resolver turns raw
// per-frame edges into `Fired` signals; the router dispatches them through this
// scene's handler chain. Consumed read-only — the bus is proven, this scene builds
// on it.
use flicker_input_core::{Fired, Resolver};
use flicker_input_router::{apply_context_requests, InputEvent, InputHandler, RouteCtx, Router};

/// The bench's COMPOSITION lives in `ui_templates.json` as this proto; the scene only
/// configures it (canonical pattern, rule E5AFBBAB — the Quartermaster is the reference).
/// There is deliberately **no `hud_assetpipeline.lua`**: a per-scene script that composed
/// the surface out of `Cell`/`Row`/`Stack` builders would put composition back in code,
/// which is the exact break ruled unacceptable in E804E0EF — just in Lua rather than Rust.
const BENCH_TEMPLATE: &str = "clayworks_bench";

/// Layout + `$token` styles live in the shared `ui_elements.json` — the ONE global
/// UI-element definition + Prism palette every prism-alpha scene reads — under the
/// `assetpipeline` key. NOT a per-scene copy: a second file would need its own
/// `theme.tokens`, forking the palette, which the one-colour-source rule forbids.
const HUD_UI_ELEMENTS: &str =
    concat!(env!("CARGO_MANIFEST_DIR"), "/../../../content/sensorium/resources/ui_elements.json");

/// Skeleton overlay colours, matching the paperdoll's rig view so the two tools read alike.
const JOINT: [f32; 4] = [0.35, 0.9, 1.0, 1.0];
/// The SELECTED joint ball — amber, the same "this one is being edited" role the paperdoll uses,
/// so a clicked joint reads apart from the cyan of the rest.
const GIZMO_SEL: [f32; 4] = [1.0, 0.8, 0.15, 1.0];
/// The translate gizmo's arrow length, as a fraction of the model radius — long enough to grab in
/// the perspective view without swamping a small joint.
const GIZMO_ARROW_FRAC: f32 = 0.18;
/// Skeleton rig-view sizing. Joint balls scale to each joint's BONE LENGTH (so extremity joints read
/// small) × `BALL_LEN_FRAC`, clamped to `[BALL_MIN_FRAC, BALL_MAX_FRAC]` of the model radius. Bones
/// draw as octahedral "diamonds" whose waist sits `BONE_WAIST_FRAC` of the bone length from the head.
const BALL_LEN_FRAC: f32 = 0.14;
const BALL_MIN_FRAC: f32 = 0.006;
const BALL_MAX_FRAC: f32 = 0.035;
const BONE_WAIST_FRAC: f32 = 0.12;

/// How an active joint drag maps cursor motion. Both drag in a PLANE whose normal is the view
/// direction (so the joint follows the cursor on screen); they differ in EFFECT:
/// - `Deform` — the PERSPECTIVE spring-back test: writes an ephemeral [`BoneOffset`] so the mesh
///   deforms while you hold, and RESTORES `restore` on release (the joint snaps back to rest).
/// - `Reposition` — an ORTHO edit: moves the rest skeleton and re-binds (permanent, mesh static).
#[derive(Clone, Copy)]
enum DragMode {
    Deform { normal: Vec3, restore: BoneOffset },
    Reposition { normal: Vec3 },
}

/// Attach markers: aged bronze for the set, rune-blue for the one being edited — the design's
/// own distinction, and the same two roles the Prism palette gives them elsewhere.
const MARKER: [f32; 4] = [0.722, 0.592, 0.353, 0.85];
const MARKER_SEL: [f32; 4] = [0.435, 0.592, 1.0, 1.0];
/// The stage floor lattice — faint enough to read as ground without competing with the rig.
const GROUND: [f32; 4] = [0.55, 0.63, 0.75, 0.16];
/// The auto-fit collision overlay — green, matching the paperdoll's collision view so the two tools
/// read alike (per-bone capsules + leaf-bone "joint ball" spheres, wireframed on the posed rig).
const COLLISION: [f32; 4] = [0.25, 1.0, 0.45, 0.9];

/// The fitting body reads as cool, dim CLAY — it is the REFERENCE, deliberately recessive.
/// Untextured in the viewport (the preview draws flat meshes), so without this the body and the
/// piece shade identically and you cannot tell where one ends and the other begins.
const BODY_TINT: [f32; 4] = [0.40, 0.44, 0.52, 1.0];
/// A CHARACTER being rigged is the subject, not a reference — so its untextured fallback is a
/// neutral, evenly-lit clay rather than the recessive [`BODY_TINT`]. Ignored the moment it ships
/// maps: a textured draw is left neutral so the real skin reads.
const SUBJECT_TINT: [f32; 4] = [0.80, 0.79, 0.77, 1.0];
/// The imported piece — warm bronze against that cool clay. Opposed in HUE, not just brightness,
/// so the two stay separable under any lighting angle and in the flat ortho panels.
const PIECE_TINT: [f32; 4] = [1.0, 0.74, 0.40, 1.0];


/// Vertex ceiling for the fitting body's reference mesh. `fitting_base` prefers the ~3.3k-tri
/// `GolemBase_Low`, but falls back to the 95k-tri authoring cut (49.6 MB) — and beyond this the
/// upload cost at `enter` outweighs a reference the user can already read from the skeleton.
const BASE_MESH_BUDGET: usize = 150_000;

/// The wizard's spine is the shared **Workflow runtime** (`flicker-widgets` `workflow.rs`):
/// a LINEAR step list loaded from [`UI_WORKFLOWS`], gated by the step DOCUMENT the scene
/// publishes each frame ([`AssetPipeline::wf_doc`]). These are the two definitions the
/// Task page's cards dispatch between — branching lives BETWEEN definitions, never inside
/// one, so the old in-spine "skip Attach for a non-character" conditional is now simply a
/// definition without the step.
///
/// `task` is the ENTRY page of BOTH — a workflow-selection card grid (Import Character /
/// Accessory / Prop / Animation). The user DECLARES the workflow there rather than the tool
/// guessing it; choosing a card opens the folder dialog, and ingest → parse → classify →
/// conform all run inline (see `open`), so the asset lands DIRECTLY on the rig-edit view.
/// The old Load / Analyze / Classify stops are gone: each was a page whose only action was
/// "click Next", so the work now happens automatically and the user is not made to walk
/// through a stage that asks nothing of them.
const WF_CHARACTER: &str = "import_character";
const WF_PROP: &str = "import_prop";

/// The workflow DEFINITIONS — durable content beside `ui_templates.json`, embedded like
/// the walker's own component modules so the wizard cannot ship without its spine.
const UI_WORKFLOWS: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../../content/sensorium/resources/ui_workflows.json"
));

/// What the Conform stage IS for the loaded asset.
///
/// Conform is ONE step in the rail carrying SEVERAL ROLES, because "rig this" means something
/// different per class: a character maps its skeleton onto the canonical 66 bones, a prop/garment
/// binds to a mount socket, an animation adjusts its clips. Before this the page was the character
/// path only, so a prop reached it and found an empty bone map with four sliders addressing
/// nothing — a dead page it had to walk past.
///
/// A prop's "rig" is not a new mechanism: it IS the attach binding (socket + placement) that the
/// fit controls already authored, and that `RigFile.attach` already stores. This type only decides
/// WHICH controls the page shows. Adding a fourth role is a variant plus its arms — the rail, the
/// navigation, the gating and `can_advance` all read this instead of re-deriving from the class.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum ConformRole {
    /// Skin: map the source skeleton to the canon and tune per-bone offsets.
    Skeleton,
    /// Prop / garment: mount socket + placement (offset / rotation / per-axis scale).
    Mount,
    /// Animation: clip adjustment. NOT wired — the page says so rather than showing controls
    /// that address nothing, which is the trap this whole type exists to close.
    Clip,
}

impl ConformRole {
    /// The role for a confirmed class. An unclassified asset takes the character path, matching
    /// every other `Some(Skin) | None` gate in the editor.
    fn of(class: Option<AssetClass>) -> Self {
        match class {
            Some(AssetClass::Prop) => Self::Mount,
            Some(AssetClass::Animation) => Self::Clip,
            Some(AssetClass::Skin) | None => Self::Skeleton,
        }
    }

    /// Title `$token` (inspector panel + footer) — a prop must never read the character
    /// "Rig" label over a mount page. The Skeleton/Mount arms reuse the rail's own
    /// `wf_step_*` tokens (one copy source); Clip's rail chip never exists, so it
    /// carries its own token. Resolved at the publish sites.
    fn title(self) -> &'static str {
        match self {
            Self::Skeleton => "$wf_step_rig",
            Self::Mount => "$wf_step_mount",
            Self::Clip => "$ap_clips_title",
        }
    }

    /// Footer hint `$token` — what the user is meant to DO on the page in this role.
    fn hint(self) -> &'static str {
        match self {
            Self::Skeleton => "$ap_map_the_source_skeleton_to_the_internal",
            Self::Mount => "$ap_bind_the_piece_to_a_socket_then_place_it",
            Self::Clip => "$ap_clip_adjustment_is_not_wired_yet_this_as",
        }
    }
}

/// The WORKING MODEL — the one skeleton the editor owns, from Analyze onward.
///
/// Conform mutates `model` in place (rename → derive → reorient → infer) and the viewport
/// frames are re-derived from it; there is deliberately no second copy of the skeleton to drift
/// against. `verts`/`tris` are measured once at parse and unchanged by conform.
struct Parsed {
    model: RawModel,
    verts: usize,
    tris: usize,
    /// Rest-pose world frames + parent topology, for the viewport skeleton. Cached — rebuilt
    /// when the model or an authored offset changes, never per frame.
    globals: Vec<Mat4>,
    parents: Vec<i32>,
    /// Bounding centre. The quad cameras all target the ORIGIN, which in Z-up ground reckoning is
    /// the asset's FEET — so the viewport draws everything offset by `-centre` to frame the asset.
    centre: Vec3,
    /// Half-extent about `centre`, to frame the orthographic views.
    radius: f32,
    /// The asset's feet plane in RECENTRED space (negative) — where the stage floor is drawn.
    floor: f32,
    /// Auto-fit collision volumes (per-bone capsules + leaf-bone spheres), rebuilt with the pose so
    /// the `Collision` overlay shows the coverage the rig currently produces. Empty for a bone-less
    /// prop. The SAME `flicker-mechanics` auto-fit the paperdoll and the runtime bridge use.
    collision: Vec<Volume>,
}

impl Parsed {
    fn new(model: RawModel) -> Self {
        let verts = model.vertices.len();
        let tris = model.indices.len() / 3;
        let mut p = Self {
            model,
            verts,
            tris,
            globals: Vec::new(),
            parents: Vec::new(),
            centre: Vec3::ZERO,
            radius: 1.0,
            floor: 0.0,
            collision: Vec::new(),
        };
        p.rebuild(&[]);
        p
    }

    fn bones(&self) -> usize {
        self.model.bones.len()
    }

    /// Re-derive the world frames, applying the editor's authored per-bone offsets on top of the
    /// conformed rest pose. `offsets` is empty until the Conform stage authors any.
    fn rebuild(&mut self, offsets: &[BoneOffset]) {
        let (globals, parents) = rest_globals(&self.model, offsets);
        let (centre, radius, floor) = model_bounds(&self.model, &globals);
        self.centre = centre;
        self.radius = radius;
        self.floor = floor;
        // Auto-fit the collision coverage from the SAME topology + rest frames the overlay draws, so
        // toggling `Collision` shows the capsules/spheres this pose would produce. Rebuilt with the
        // pose (cheap) rather than once, so an authored bone offset moves its volume too.
        self.collision = autofit_capsules_from(&parents, &globals);
        self.globals = globals;
        self.parents = parents;
    }

    /// Index of a bone by canonical name — how an attach point finds its parent.
    fn bone_index(&self, name: &str) -> Option<usize> {
        self.model.bones.iter().position(|b| b.name == name)
    }
}

/// One bone's editor-authored correction, applied on top of the conform result. This is the
/// AUTHORED data; the posed skeleton is derived from it, so "Reset bone" is just zeroing it.
#[derive(Clone, Copy, Default, PartialEq)]
struct BoneOffset {
    /// Translation in source units (cm), parent-relative — the same space as `RawBone::translation`.
    t: [f32; 3],
    /// Roll about the bone's own X axis, in degrees.
    roll: f32,
}

impl BoneOffset {
    fn is_zero(&self) -> bool {
        *self == Self::default()
    }
}

/// How a bone came to be in the conformed rig — what colours its row in the bone map.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum MapState {
    /// Carried over from the source and renamed to a canonical name.
    Ok,
    /// Placed by a derive pass whose result is worth a human's eye (hip / shoulder / ankle).
    Review,
    /// Not in the source at all — inferred from the reference rig.
    Auto,
}

impl MapState {
    /// The dotted colour path its row dot reads, and the tag it carries.
    fn color(self) -> &'static str {
        match self {
            MapState::Ok => "assetpipeline.map.ok",
            MapState::Review => "assetpipeline.map.review",
            MapState::Auto => "assetpipeline.map.auto",
        }
    }

    /// Row tag `$token` — resolved where the bone row is composed.
    fn tag(self) -> &'static str {
        match self {
            MapState::Ok => "$ap_tag_mapped",
            MapState::Review => "$ap_tag_review",
            MapState::Auto => "$ap_tag_auto",
        }
    }
}

/// The conform result plus what the editor authored on top of it.
struct Rig {
    rename: RenameReport,
    out: ConformOutput,
    /// Per-bone provenance, parallel to the working model's bones.
    map: Vec<MapState>,
    /// Per-bone authored corrections, parallel to the working model's bones.
    offsets: Vec<BoneOffset>,
    /// Selected row in the bone map.
    sel: usize,
    /// Whether a joint is FOCUSED — has the gizmo and receives ortho nudges. Click a joint (Perspective,
    /// or Ctrl-click an ortho view) or a bone-map row to focus; click empty space to deselect.
    focused: bool,
    /// First visible row — the bone list is 66 rows in a 150px box, so it pages.
    window: usize,
}

impl Rig {
    fn counts(&self) -> (usize, usize, usize) {
        let n = |s: MapState| self.map.iter().filter(|m| **m == s).count();
        (n(MapState::Ok), n(MapState::Review), n(MapState::Auto))
    }
}

/// One authored attach point: a named socket at an offset from a real canonical bone.
///
/// The parent bones are all canonical (`hand_r`, `thigh_l`, `spine_02`, …), so a point is fully
/// defined against the conformed skeleton. Persisting the SET of them is what `flicker.rig` cannot
/// carry yet — its `attach` block is a single mount describing how one asset hangs off a socket,
/// not a list of sockets a character offers. Review reports that gap rather than papering over it.
struct AttachPoint {
    id: &'static str,
    label: &'static str,
    parent: &'static str,
    offset: [f32; 3],
    /// The parent's index in the working model, resolved ONCE when the rig gains canonical names.
    /// Looking it up by name per frame would be 6 points × 65 string compares every frame, in a
    /// panel that only changes when conform runs.
    bone: Option<usize>,
}

/// The six points the design specifies, in rail order. Labels are `$token`s,
/// resolved where the attach rows are composed (the Model-channel strings gate).
const ATTACH_POINTS: [(&str, &str, &str); 6] = [
    ("hand_r", "$ap_grip_hand_r", "hand_r"),
    ("hand_l", "$ap_grip_hand_l", "hand_l"),
    ("holster_r", "$ap_holster_hip_r", "thigh_r"),
    ("holster_l", "$ap_holster_hip_l", "thigh_l"),
    ("scabbard", "$ap_scabbard_back", "spine_02"),
    ("belt", "$ap_belt_waist", "pelvis"),
];

/// Candidate mount sockets a PROP or GARMENT can hang from — the body bones the fit stage offers
/// as its picker (a non-character asset mounts to ONE socket, unlike the character's six points).
/// Curated to the common canonical bones + the dedicated `Weapon_R/L` sockets; the choice is
/// validated against the loaded base body at bake time, so a missing bone surfaces as a commit
/// error rather than a silent mis-mount.
const SOCKETS: &[(&str, &str)] = &[
    // (canonical bone, display-label `$token` — resolved where the picker rows compose)
    ("hand_r", "$ap_hand_r"),
    ("hand_l", "$ap_hand_l"),
    ("Weapon_R", "$ap_weapon_socket_r"),
    ("Weapon_L", "$ap_weapon_socket_l"),
    ("spine_02", "$ap_chest"),
    ("spine_03", "$ap_upper_chest"),
    ("pelvis", "$ap_pelvis"),
    ("neck_01", "$ap_neck"),
    ("head", "$ap_head"),
    ("clavicle_l", "$ap_shoulder_l"),
    ("thigh_r", "$ap_thigh_r"),
    ("thigh_l", "$ap_thigh_l"),
    ("calf_l", "$ap_shin_l"),
    ("calf_r", "$ap_shin_r"),
    ("foot_l", "$ap_foot_l"),
    ("foot_r", "$ap_foot_r"),
    ("lowerarm_l", "$ap_forearm_l"),
    ("lowerarm_r", "$ap_forearm_r"),
];

/// A prop/garment's authored placement — the human-in-the-loop fit the Attach stage tunes for a
/// NON-character asset (Skin uses the six attach points + per-bone offsets instead). `socket`
/// indexes [`SOCKETS`]; `rot` is euler degrees; `scale` is PER-AXIS and `uniform` is scale-all
/// (the paperdoll fit gadget's X/Y/Z + scale-all, which the rig format already carried). Baked into
/// the rig's `attach` block (prop) or the skin transform (garment) at Commit — what the user
/// approved is what ships.
#[derive(Clone, Copy)]
struct PropFit {
    socket: usize,
    offset: [f32; 3],
    rot: [f32; 3],
    scale: [f32; 3],
    uniform: f32,
}

impl Default for PropFit {
    fn default() -> Self {
        Self { socket: 0, offset: [0.0; 3], rot: [0.0; 3], scale: [1.0; 3], uniform: 1.0 }
    }
}

impl PropFit {
    fn socket_name(&self) -> &'static str {
        SOCKETS.get(self.socket).map(|(id, _)| *id).unwrap_or("pelvis")
    }
}

/// The loaded source folder — what Load produced, plus what each later stage added.
struct Source {
    dir: PathBuf,
    scan: Scan,
    /// The riggable mesh chosen to rig.
    fbx: PathBuf,
    /// EVERY riggable mesh the scan found — a weapon set is four or five pieces, an outfit folder is
    /// tops/pants/gloves/shoes — plus which one is selected. The Load stage offers the choice rather
    /// than refusing the folder; only a single-mesh folder skips straight past it.
    candidates: Vec<PathBuf>,
    candidate_sel: usize,
    /// Top row of the picker's visible window (it pages, like the bone map and socket list).
    pick_window: usize,
    textures: usize,
    parsed: Option<Parsed>,
    /// What Classify detected, and the override the user may have applied over it.
    report: Option<AssetReport>,
    class: Option<AssetClass>,
    prop: PropKind,
    /// What Conform produced — `None` until the stage runs.
    rig: Option<Rig>,
    /// Authored attach points (always the six; `parent` resolves against the conformed rig).
    attach: Vec<AttachPoint>,
    /// Selected attach point.
    attach_sel: usize,
    /// The prop/garment mount fit — socket + offset/rotation/scale — authored in the Attach stage
    /// for a non-character asset. Unused by the Skin path (which uses `attach` + bone offsets).
    fit: PropFit,
    /// The top row of the socket picker's visible window (it pages [`SOCKETS`] six at a time,
    /// exactly like the bone map pages the skeleton).
    fit_window: usize,
    /// Where Commit wrote the rig, once it has.
    committed: Option<PathBuf>,
    /// Set when a stage failed, shown in the inspector instead of a fabricated result.
    error: Option<String>,
}

impl Source {
    /// The asset name the pipeline would bake under — the source folder's own name.
    fn asset_name(&self) -> &str {
        self.dir.file_name().and_then(|s| s.to_str()).unwrap_or("asset")
    }

    fn file_name(&self) -> &str {
        self.fbx.file_name().and_then(|s| s.to_str()).unwrap_or("")
    }

    /// The effective classification: the user's override if they made one, else what was detected.
    fn class(&self) -> Option<AssetClass> {
        self.class.or(self.report.as_ref().map(|r| r.class))
    }

    /// World position of an attach point — its parent bone's conformed frame plus the authored
    /// offset. `None` while the point has no parent bone (before conform runs, the source carries
    /// vendor names, so nothing resolves).
    fn attach_world(&self, i: usize) -> Option<Vec3> {
        let p = self.attach.get(i)?;
        let g = self.parsed.as_ref()?.globals.get(p.bone?)?;
        Some(g.w_axis.truncate() + Vec3::from_array(p.offset))
    }

    /// Bind every attach point to its parent bone. Called when the working model's names change
    /// — i.e. once, after conform.
    fn resolve_attach(&mut self) {
        let Some(parsed) = self.parsed.as_ref() else { return };
        for p in &mut self.attach {
            p.bone = parsed.bone_index(p.parent);
        }
    }
}

/// Orbit camera state for the interactive perspective view.
#[derive(Clone, Copy)]
struct Orbit {
    yaw: f32,
    pitch: f32,
    /// Distance as a multiple of the model radius, so it frames any asset size.
    dist_scale: f32,
    /// Wheel zoom, multiplying the framing. `1.0` is the default framing; applied to the perspective
    /// distance AND the orthographic height. PER VIEW — the editor holds one `Orbit` per quad, so a
    /// notch zooms only the panel under the cursor.
    zoom: f32,
    /// This view's look-at point, slid across its plane by the right-drag pan. Zero frames the
    /// asset's centre; panning is what lets the user work up close on a hand or a skull.
    pan: Vec3,
}

/// Vertical field of view of the perspective view. Named because the pan needs it to convert
/// pixels to world units — if the two disagreed, panning would drift against the cursor.
const FOV_Y: f32 = 60.0 * std::f32::consts::PI / 180.0;

impl Default for Orbit {
    fn default() -> Self {
        // Start FACE-ON-ish: the eye orbits the XY plane as `(cos yaw, sin yaw, sin pitch)`, and a
        // character faces +Y, so yaw ≈ π/2 looks at its front. Backed off a little (1.25) for a
        // three-quarter view, which reads the silhouette better than dead-on, and lifted slightly.
        Self { yaw: 1.25, pitch: 0.22, dist_scale: 2.4, zoom: 1.0, pan: Vec3::ZERO }
    }
}

impl Orbit {
    /// Eye distance for a model of `radius` — the ONE place the framing multipliers are applied,
    /// so the camera and the pan's pixel scale cannot disagree.
    fn dist(&self, radius: f32) -> f32 {
        (radius * self.dist_scale * self.zoom).max(1.0)
    }

    /// The framing radius the ORTHOGRAPHIC views should be built with, so the wheel zooms all
    /// four panels together instead of only the perspective one.
    fn ortho_radius(&self, radius: f32) -> f32 {
        radius * self.zoom
    }

    /// Wheel zoom. MULTIPLICATIVE, so a notch is a constant proportion and zooming feels the same
    /// whether you are framed on a whole body or already close on a hand — an additive step would
    /// crawl when far out and jump when near. Clamped so the wheel can never invert or escape.
    fn zoom_by(&mut self, wheel: f32) {
        if wheel == 0.0 || !wheel.is_finite() {
            return;
        }
        self.zoom = (self.zoom * (1.0 - wheel * 0.12)).clamp(0.05, 6.0);
    }

    /// The eye's offset from the look-at point. Z-up source content: orbit in the XY plane,
    /// elevation on Z.
    fn eye_offset(&self, radius: f32) -> Vec3 {
        let (sy, cy) = self.yaw.sin_cos();
        let cp = self.pitch.cos();
        Vec3::new(cy * cp, sy * cp, self.pitch.sin()) * self.dist(radius)
    }

    fn camera(&self, radius: f32) -> Camera {
        let r = self.dist(radius);
        Camera {
            position: self.pan + self.eye_offset(radius),
            target: self.pan,
            up: Vec3::Z,
            fov_y_radians: FOV_Y,
            near: 0.01,
            // Measured from the look-at point, so a panned-away camera cannot clip the asset.
            far: r * 12.0 + self.pan.length(),
            ortho_height: None,
        }
    }

    /// Slide THIS view's look-at point across its own plane — the right-drag pan. Each quad owns its
    /// `Orbit`, so a pan moves only the panel under the cursor; the others stay put.
    ///
    /// Takes the CAMERA rather than an angle so the basis comes from the view actually being dragged:
    /// dragging in TOP pans across XY, in FRONT across XZ. Deriving it from the orbit angles instead
    /// would pan along the PERSPECTIVE plane, which feels broken in an orthographic panel.
    ///
    /// Scaled so the content tracks the cursor **1:1, at any zoom**: an orthographic camera states
    /// its visible height outright, a perspective one's is `2·dist·tan(fov/2)` at the look-at
    /// depth. A fixed per-pixel constant (as the orbit uses for angles) would crawl on a large
    /// asset and bolt on a small one, since both heights scale with the model radius.
    fn pan_by_view(&mut self, delta: Vec2, cam: &Camera, viewport_h: f32) {
        if viewport_h <= 0.0 {
            return;
        }
        let forward = (cam.target - cam.position).normalize_or_zero();
        let right = forward.cross(cam.up).normalize_or_zero();
        let up = right.cross(forward).normalize_or_zero();
        let visible_h = match cam.ortho_height {
            Some(h) => h,
            None => 2.0 * (cam.position - cam.target).length() * (cam.fov_y_radians * 0.5).tan(),
        };
        let world_per_px = visible_h / viewport_h;
        // The CONTENT follows the cursor, so the look-at point moves the OPPOSITE way; screen Y
        // grows downward, so dragging down raises the target and the asset slides down with it.
        self.pan += (-right * delta.x + up * delta.y) * world_per_px;
    }
}

/// The editor scene.
pub struct AssetPipeline {
    /// The loaded workflow DEFINITIONS ([`UI_WORKFLOWS`]) — the dispatch table the Task
    /// cards choose from (Character → [`WF_CHARACTER`], Prop/Accessory/Animation →
    /// [`WF_PROP`]; ruling: branching lives BETWEEN linear definitions).
    workflows: HashMap<String, WorkflowDef>,
    /// The ACTIVE definition — read for rail-chip membership (`wf_<id>_show`) and to
    /// restart the same wizard for the next piece of a set.
    wf_def: WorkflowDef,
    /// The RUNNING workflow — the shared runtime owning step ordinal, needs-gating, the
    /// Back discard guard and the step surfaces. The scene feeds it results + the step
    /// document each frame and publishes its UI state into the HUD Model.
    wf: Workflow,
    /// The workflow the user DECLARED on the Task page — the class Load stamps onto the source
    /// instead of auto-detecting it. `None` before a card is chosen (and on the character/default
    /// path, which `ConformRole::of(None)` already treats as Skeleton).
    pending_class: Option<AssetClass>,
    /// The Prop SUB-TYPE the user DECLARED on the Import panel — the clothing-vs-static fork that
    /// decides the bake path (`Clothing` → `write_garment`; anything else → `write_prop`). Set by
    /// the Accessory/Prop cards; `None` (→ `PropKind::Accessory` default) for Character/Animation.
    pending_prop: Option<PropKind>,
    source: Option<Source>,
    grid: Option<QuadGrid>,
    /// The framed holder rect the HUD reserves for the 2×2 (the `editor_quad` `stage` node). The
    /// scene tiles the `QuadGrid` inside exactly this rect, so the viewport, the composite and the
    /// pointer-picking all agree; `None` until the HUD has laid out its first frame.
    quad_rect: Option<Rect>,
    /// One camera per quad, in `EDITOR_QUADS` order (PERSP, TOP, SIDE, FRONT). Each view pans and
    /// zooms independently — a viewport control acts only on the panel under the cursor. Only view 0
    /// (PERSP) uses yaw/pitch; the ortho views are fixed axes and read only `pan` + `zoom`.
    orbits: [Orbit; 4],
    show_skeleton: bool,
    /// Cached last cursor, for orbit dragging.
    last_mouse: Vec2,
    // ── Pause plumbing, as the shell expects (built in `enter`, handed to PauseScene). ──
    /// Per-context action maps (spec section 9): the `World` base is `wasd_and_mouse`
    /// — the map the resolver reads and that each pushed `PauseScene` clones. This
    /// scene has no extra contexts (no text entry / no modal map), so `World` is the
    /// whole stack and `active()` is always `World`.
    bindings: ContextualBindings,
    controls: AbstractControls,
    gamepad_config: GamepadConfig,
    /// The input seam (spec section 5/9): a stateful edge `Resolver` (replaces the
    /// hand-rolled `menu_prev` bool), a REUSED `Fired` scratch buffer (no per-frame
    /// alloc — RT-7), and the router's request queue. `tick` is the resolver's
    /// monotonic `TickTime` — a frame counter, NOT wall-clock (spec section 3.2a).
    resolver: Resolver,
    ev: Vec<Fired>,
    route: RouteCtx,
    tick: u64,
    ui_theme: Option<Theme>,
    // ── HUD (component walker) ──
    /// The HUD script host, RETAINED so the walker can dispatch each control's draw/hit
    /// to its `ui/<kind>.lua` component module (`run_ui_with`). `None` when the script
    /// failed to load — component nodes then draw only their structural box (or
    /// nothing): a visible failure, not an alternate render path.
    ui_host: Option<ScriptHost>,
    /// The template registry, built ONCE and held — `build_tree` expands against it
    /// every frame, which is what keeps template params live (CM4 7A1F32C5).
    templates: TemplateRegistry,
    /// The screen's declarative signal bindings (S9), collected from the expanded
    /// tree's ROOT `on_<signal>` props (`on_menu = "pause_open"`). Refreshed with the
    /// tree each frame; the walker layer consumes a declared signal and `update` maps
    /// the fired name onto its scene action.
    ui_intents: UiIntents,
    /// Intent names fired last frame — republished ONCE into the next HUD Model
    /// as the transient `sig_<name>` mirror (S9), then dropped.
    fired_sigs: Vec<String>,
    ui_state: UiState,
    ui_styles: serde_json::Value,
    hud_commands: Vec<HudCommand>,
    hud_white: Option<TextureHandle>,
    // ── Prop/garment fit PREVIEW: the base rig loaded once (socket frames + skeleton overlay) and
    // the imported asset mesh uploaded once, so the fit stage SHOWS the piece placed on the body —
    // the human-in-the-loop verification, not a blind slider. Unused by the character path. ──
    base: Option<BasePreview>,
    /// The fitting body's mesh on the GPU. Uploaded ONCE in `enter` — not lazily on first fit —
    /// so turning the reference body on for a prop or outfit is instant. Textured with the body's
    /// own maps when it ships them, else the flat [`BODY_TINT`] clay.
    base_upload: Option<Uploaded>,
    /// Decoded texture maps, keyed by source path and shared across every upload that references
    /// the same file — so re-uploading the piece (a turn of ROT, a different candidate) never
    /// re-decodes its maps. See [`load_map`].
    textures: HashMap<PathBuf, TextureHandle>,
    /// Draw the reference BODY behind the piece being fitted. On by default: placement is judged
    /// against a shape, not a stick figure.
    show_base: bool,
    /// Draw the auto-fit COLLISION overlay (per-bone capsules + leaf "joint ball" spheres) over the
    /// posed rig — the `Display` panel's "Collision" toggle, off by default (a diagnostic, not the
    /// default read). The volumes themselves live on [`Parsed::collision`].
    show_collision: bool,
    /// What the perspective view was framed on last draw. Cached rather than re-derived so the
    /// right-drag pan converts pixels to world units at exactly the scale the CAMERA used — the
    /// framing precedence (base body while fitting, else the parsed asset) lives in one place.
    view_radius: f32,
    /// The uploaded asset mesh, keyed by the IDENTITY of the model it was built from: the folder,
    /// WHICH piece was picked, and its orientation. Keying on the folder alone meant turning the
    /// asset (or picking another piece in the same folder) silently kept showing the stale upload —
    /// the "ROT buttons do nothing" bug. Any change re-uploads.
    preview: Option<(Uploaded, PreviewKey)>,
    /// The LIVE CPU-skinned character mesh for the Conform stage — re-skinned and re-uploaded when
    /// the authored pose (`pose_gen`) changes, so dragging a joint deforms the mesh in view. Keyed by
    /// the source identity AND `pose_gen`; other stages draw the cheap rest mesh via `preview`.
    skinned: Option<(Uploaded, (PreviewKey, u64))>,
    /// Bumped on every authored-offset write (a Conform slider or a gizmo drag) — the skinned-mesh
    /// cache key, so the live re-skin re-uploads exactly when the pose changes and never otherwise.
    pose_gen: u64,
    /// The in-scene transform gizmo's mode (Translate is the only one that drags in slice 1; Rotate
    /// and Scale draw their handles but are inert). Toggled from the HUD.
    gizmo_mode: GizmoMode,
    /// The axis handle being dragged and the pick ray from the PREVIOUS frame — the drag projects the
    /// new cursor ray against it to get the world delta. `None` when no handle is grabbed.
    gizmo_drag: Option<(DragMode, (Vec3, Vec3))>,
    /// Symmetry: an ortho reposition of a left/right joint mirrors to its `_l`/`_r` twin. Default on;
    /// a HUD toggle turns it off to move one joint alone.
    mirror_joints: bool,
}

/// What a cached preview upload was built from. Any difference means the GPU copy is stale.
type PreviewKey = (PathBuf, usize);

/// One uploaded preview mesh. TEXTURED when its source resolved an albedo map, else the flat
/// tinted fallback — `flicker-render` keeps the two pipelines in separate stores with distinct
/// handle types, so a mesh is one or the other and never both.
///
/// The tint is why the split matters: an UNtextured body and piece are told apart only by
/// [`BODY_TINT`] vs [`PIECE_TINT`], but a textured draw is left NEUTRAL so the user sees the
/// vendor's actual maps — which is the whole point of previewing them.
#[derive(Clone, Copy)]
enum Uploaded {
    Textured { mesh: TexturedMeshHandle, albedo: TextureHandle, maps: PbrMaps },
    Flat(MeshHandle),
}

impl Uploaded {
    /// Draw at `world`. `flat_tint` applies ONLY to the untextured path (see the type docs).
    fn draw(self, r: &mut Renderer, world: Mat4, flat_tint: [f32; 4]) {
        match self {
            Uploaded::Textured { mesh, albedo, maps } => {
                r.draw_textured_mesh_pbr(mesh, albedo, maps, world, MeshDrawOptions::default())
            }
            Uploaded::Flat(h) => {
                r.draw_mesh(h, world, MeshDrawOptions { tint: flat_tint, ..Default::default() })
            }
        }
    }

    /// Release the GPU mesh. The map TEXTURES are deliberately not freed here — they live in the
    /// editor's path-keyed cache, shared by every upload referencing the same file.
    fn free(self, r: &mut Renderer) {
        match self {
            Uploaded::Textured { mesh, .. } => r.free_textured_mesh(mesh),
            Uploaded::Flat(h) => r.free_mesh(h),
        }
    }
}

/// Decode one PNG map and upload it, memoised BY PATH — re-picking a candidate or turning the
/// orientation control re-uploads the mesh on every key, and without this cache each of those
/// would re-decode (and leak) the very same maps.
///
/// `srgb` for colour data (the albedo); the PBR maps are LINEAR, exactly as the paperdoll loads
/// them. A map that fails to decode leaves its slot at the pipeline's 1×1 default.
fn load_map(
    r: &mut Renderer,
    cache: &mut HashMap<PathBuf, TextureHandle>,
    path: &Path,
    srgb: bool,
) -> Option<TextureHandle> {
    if let Some(h) = cache.get(path) {
        return Some(*h);
    }
    match image::open(path) {
        Ok(img) => {
            let rgba = img.to_rgba8();
            let (w, h) = rgba.dimensions();
            let handle = if srgb {
                r.load_texture(rgba.as_raw(), w, h)
            } else {
                r.load_texture_linear(rgba.as_raw(), w, h)
            };
            cache.insert(path.to_path_buf(), handle);
            tracing::info!(map = %path.display(), w, h, srgb, "asset pipeline: texture loaded");
            Some(handle)
        }
        Err(e) => {
            tracing::warn!(map = %path.display(), "asset pipeline: texture failed ({e}); using the default");
            None
        }
    }
}

/// Upload one preview mesh — textured when `maps` resolves an albedo AND the geometry carries UVs,
/// else flat.
///
/// `indices` may share vertices, but [`build_textured_verts`] assigns ONE tangent per consecutive
/// triple, so the textured path DE-INDEXES into a flat triangle list first. The flat path keeps the
/// index buffer as-is — it needs no tangent basis.
fn upload_preview(
    r: &mut Renderer,
    cache: &mut HashMap<PathBuf, TextureHandle>,
    maps: &SourceMaps,
    verts: &[MeshVertex],
    uvs: &[[f32; 2]],
    indices: &[u32],
) -> Uploaded {
    // The converter emits no index list when the vertices are already sequential.
    let seq: Vec<u32>;
    let idx: &[u32] = if indices.is_empty() {
        seq = (0..verts.len() as u32).collect();
        &seq
    } else {
        indices
    };

    // No albedo — or no UVs to sample one with — falls back to the flat tinted path, unchanged.
    let albedo = maps
        .base_color
        .as_deref()
        .filter(|_| uvs.len() == verts.len())
        .and_then(|p| load_map(r, cache, p, true));
    let Some(albedo) = albedo else {
        return Uploaded::Flat(r.upload_mesh(verts, MeshIndices::U32(idx)));
    };

    let flat: Vec<usize> = idx.iter().map(|&i| i as usize).filter(|&i| i < verts.len()).collect();
    let tv = build_textured_verts(
        0..flat.len(),
        |k| verts[flat[k]].position,
        |k| verts[flat[k]].normal,
        |k| uvs[flat[k]],
    );
    let li: Vec<u32> = (0..tv.len() as u32).collect();
    let mesh = r.upload_textured_mesh(&tv, MeshIndices::U32(&li));
    let normal = maps.normal.as_deref().and_then(|p| load_map(r, cache, p, false));
    let roughness = maps.roughness.as_deref().and_then(|p| load_map(r, cache, p, false));
    let metalness = maps.metalness.as_deref().and_then(|p| load_map(r, cache, p, false));
    Uploaded::Textured {
        mesh,
        albedo,
        maps: PbrMaps { normal, roughness, metalness, ao: None },
    }
}

/// The base reference rig, loaded once for the prop/garment fit preview: the rest skeleton (overlay
/// + framing) and each bone's `inverse_bind` (the socket world frame the piece mounts to). CPU-only.
struct BasePreview {
    names: Vec<String>,
    parents: Vec<i32>,
    /// Rest world frame per bone = `inverse(inverse_bind)`.
    globals: Vec<Mat4>,
    ibind: Vec<[f32; 16]>,
    /// Bounding centre of the body — drawn offset by `-centre` for the same reason as [`Parsed`].
    centre: Vec3,
    /// Half-extent about `centre`, to frame the orthographic views on the body.
    radius: f32,
    /// The body's feet plane in RECENTRED space (negative) — the stage floor under the fit preview.
    floor: f32,
    /// The fitting body's MESH, CPU-side, uploaded once by `enter`. Empty when the rig carries no
    /// mesh or it exceeds [`BASE_MESH_BUDGET`] — then only the skeleton is shown.
    verts: Vec<MeshVertex>,
    /// Parallel to `verts` — the body's UVs, so the reference body can preview TEXTURED.
    uvs: Vec<[f32; 2]>,
    indices: Vec<u32>,
    /// The body's OWN maps, resolved beside its rig, so the reference previews with the texture set
    /// it actually ships with instead of as untextured clay.
    maps: SourceMaps,
}

impl BasePreview {
    /// Load the canonical base rig for the preview — best-effort (`None` when it is absent, so the
    /// editor still runs without a content tree, exactly as the tests skip).
    fn load() -> Option<Self> {
        // Loads `fitting_base` (the clay Golem), not the conform canon.
        //
        // The MESH comes too, because it is the reference body a prop/garment is fitted AGAINST:
        // judging whether hair sits on the skull, or a blade in the grip, needs a SHAPE — a bare
        // skeleton gives the eye almost nothing to place against. `fitting_base` prefers the
        // ~3.3k-tri `GolemBase_Low` (1.7 MB), so this is cheap; the 95k-tri authoring cut is the
        // fallback and is caught by the budget below rather than stalling the scene.
        #[derive(serde::Deserialize)]
        struct BaseRig {
            #[serde(default)]
            skeleton: flicker_skeletal::format::Skeleton,
            #[serde(default)]
            mesh: flicker_skeletal::format::Mesh,
        }
        let base_path = fitting_base();
        // Gz-transparent read: the fitting body ships gz-at-rest in the package.
        let text = flicker_content::package::read_text(&base_path).ok()?;
        let rig: BaseRig = serde_json::from_str(&text).ok()?;
        if rig.skeleton.bones.is_empty() {
            return None;
        }
        let names: Vec<String> = rig.skeleton.bones.iter().map(|b| b.name.clone()).collect();
        let parents: Vec<i32> = rig.skeleton.bones.iter().map(|b| b.parent).collect();
        let ibind: Vec<[f32; 16]> = rig.skeleton.bones.iter().map(|b| b.inverse_bind).collect();
        let globals: Vec<Mat4> = ibind.iter().map(|m| Mat4::from_cols_array(m).inverse()).collect();

        // Over budget → fall back to the skeleton rather than stall `enter` on a 50 MB upload. The
        // reference body is a nicety; the bone frames are the contract.
        let too_dense = rig.mesh.vertices.len() > BASE_MESH_BUDGET;
        if too_dense {
            tracing::warn!(
                verts = rig.mesh.vertices.len(),
                budget = BASE_MESH_BUDGET,
                "asset pipeline: fitting body over budget — showing its skeleton only"
            );
        }
        let (verts, uvs, indices): (Vec<MeshVertex>, Vec<[f32; 2]>, Vec<u32>) = if too_dense {
            (Vec::new(), Vec::new(), Vec::new())
        } else {
            let v: Vec<MeshVertex> = rig
                .mesh
                .vertices
                .iter()
                .map(|x| MeshVertex { position: x.p, normal: x.n, material: 0 })
                .collect();
            let uv: Vec<[f32; 2]> = rig.mesh.vertices.iter().map(|x| x.uv).collect();
            // The converter emits no index list when the vertices are already sequential.
            let i: Vec<u32> = if rig.mesh.indices.is_empty() {
                (0..v.len() as u32).collect()
            } else {
                rig.mesh.indices.clone()
            };
            (v, uv, i)
        };

        // The body's maps are BASENAMES in its material, written beside the rig by the same
        // `wire_textures` that bakes every asset — so resolving them against the rig's own folder
        // is all it takes to preview the reference body with its real skin.
        let dir = base_path.parent().map(Path::to_path_buf).unwrap_or_else(|| PathBuf::from("."));
        let mat = rig.mesh.materials.first();
        let named = |s: &str| (!s.is_empty()).then(|| dir.join(s));
        let maps = SourceMaps {
            base_color: mat.and_then(|m| named(&m.base_color)),
            metalness: mat.and_then(|m| named(&m.metalness)),
            roughness: mat.and_then(|m| named(&m.roughness)),
            normal: mat.and_then(|m| named(&m.normal)),
        };

        // Frame on the MESH when there is one and the bone frames otherwise — the same precedence
        // `model_bounds` uses, so the body and the imported piece are framed by ONE rule, and the
        // stage floor lands on the actual soles rather than the lowest joint.
        let (mut lo, mut hi) = (Vec3::splat(f32::INFINITY), Vec3::splat(f32::NEG_INFINITY));
        if verts.is_empty() {
            for g in &globals {
                let p = g.w_axis.truncate();
                lo = lo.min(p);
                hi = hi.max(p);
            }
        } else {
            for v in &verts {
                let p = Vec3::from(v.position);
                lo = lo.min(p);
                hi = hi.max(p);
            }
        }
        let centre = (lo + hi) * 0.5;
        let radius = ((hi - lo).max_element() * 0.5).max(50.0);
        // Recentred, exactly as `model_bounds` reports it — the sole of the foot.
        let floor = lo.z - centre.z;
        Some(Self { names, parents, globals, ibind, centre, radius, floor, verts, uvs, indices, maps })
    }

    fn socket_index(&self, name: &str) -> Option<usize> {
        self.names.iter().position(|n| n == name)
    }
}

/// One frame's prop/garment preview: the uploaded mesh, its socket-placed world matrix, and the
/// base skeleton segments to draw it against.
struct PreviewDraw {
    mesh: Uploaded,
    world: Mat4,
    base_joints: Vec<(Vec3, Vec3)>,
    radius: f32,
    /// The base body's feet plane, already recentred — the preview draws its floor there.
    floor: f32,
    /// The reference BODY to draw the piece against. `None` when the toggle is off, the rig had no
    /// mesh, or it blew the budget — the skeleton overlay still stands in for it.
    base_mesh: Option<Uploaded>,
    /// Where that body sits: the recentring the whole preview shares.
    base_world: Mat4,
}

impl Default for AssetPipeline {
    fn default() -> Self {
        Self::new()
    }
}

impl AssetPipeline {
    /// Build the editor. The HUD is not parsed here and not cached: composition is
    /// DATA (`clayworks_bench` in `ui_templates.json`) and [`Self::build_tree`] emits
    /// one instance of it every frame.
    pub fn new() -> Self {
        let ui_styles = load_styles(HUD_UI_ELEMENTS);
        // The script host is the COMPONENT LIBRARY — the `ui/<kind>.lua` modules the
        // walker calls to draw and hit each control. It is NOT a place this scene keeps
        // a tree. `None` on load failure → component nodes draw only their structural
        // box: a visible failure, not an alternate render path.
        let ui_host = match ScriptHost::library(UI_COMPONENT_MODULES) {
            Ok(host) => Some(host),
            Err(e) => {
                tracing::error!("component library load failed: {e} — no HUD");
                None
            }
        };
        // The workflow spine. The definitions are EMBEDDED content, so a parse failure is
        // a build bug the suite catches, not a runtime state — expect() over limping on
        // with no wizard. The character definition is also the pre-dispatch default:
        // `ConformRole::of(None)` already treats an unclassified asset as a character, so
        // the empty bench shows the same four-stop rail it always has.
        let workflows = workflows_from_json(UI_WORKFLOWS);
        let wf_def = workflows
            .get(WF_CHARACTER)
            .expect("ui_workflows.json ships the import_character definition")
            .clone();
        let wf = Workflow::from_def(&wf_def);
        Self {
            workflows,
            wf_def,
            wf,
            pending_class: None,
            pending_prop: None,
            source: None,
            grid: None,
            quad_rect: None,
            orbits: [Orbit::default(); 4],
            show_skeleton: true,
            last_mouse: Vec2::ZERO,
            bindings: ContextualBindings::new(InputMap::wasd_and_mouse()),
            controls: AbstractControls::default(),
            gamepad_config: GamepadConfig::default(),
            resolver: Resolver::new(),
            ev: Vec::new(),
            route: RouteCtx::new(),
            tick: 0,
            ui_theme: None,
            ui_host,
            templates: builtin_templates(),
            // Refreshed from the built tree's root each frame in `update`.
            ui_intents: UiIntents::default(),
            fired_sigs: Vec::new(),
            ui_state: UiState::new(),
            ui_styles,
            hud_commands: Vec::new(),
            hud_white: None,
            base: None,
            base_upload: None,
            textures: HashMap::new(),
            show_base: true,
            show_collision: false,
            view_radius: 100.0,
            preview: None,
            skinned: None,
            pose_gen: 0,
            mirror_joints: true,
            gizmo_mode: GizmoMode::default(),
            gizmo_drag: None,
        }
    }

    /// The six attach points, authored fresh for a newly loaded asset.
    fn new_attach() -> Vec<AttachPoint> {
        ATTACH_POINTS
            .iter()
            .map(|(id, label, parent)| AttachPoint {
                id,
                label,
                parent,
                offset: [0.0; 3],
                bone: None,
            })
            .collect()
    }

    /// LOAD — the native open-folder dialog, then `flicker-content`'s ingest scan. Errors
    /// (no riggable mesh, several of them) are surfaced, never guessed around: the scan's
    /// own disambiguation guard decides.
    fn load_folder(&mut self) {
        let Some(dir) = rfd::FileDialog::new().set_title("Open asset source folder").pick_folder()
        else {
            return; // cancelled — stay put
        };
        self.open(dir);
    }

    /// Ingest a folder that has already been chosen. Split from the dialog so the whole wizard
    /// downstream of it is exercisable without a GUI.
    /// The camera of the quad under `cursor` — so a pan works in the plane of the view being
    /// dragged. Falls back to the perspective camera when the cursor is outside the grid, or the
    /// grid has not been built yet, so a drag always has a sane basis rather than doing nothing.
    fn view_camera_at(&self, cursor: Vec2, screen: Vec2) -> Camera {
        let radius = self.view_radius;
        self.grid
            .as_ref()
            .and_then(|g| {
                g.cell_at(cursor, screen).map(|i| {
                    let o = &self.orbits[i];
                    g.camera(i, o.ortho_radius(radius), &o.camera(radius))
                })
            })
            .unwrap_or_else(|| self.orbits[0].camera(radius))
    }

    fn open(&mut self, dir: PathBuf) {
        // A new asset reframes: drop EVERY view's pan/zoom, or a fresh piece opens off-screen or at
        // the last one's magnification because a camera is still parked where it was left.
        for o in &mut self.orbits {
            o.pan = Vec3::ZERO;
            o.zoom = 1.0;
        }
        match scan_folder(&dir) {
            Ok(scan) => {
                let textures = scan.of_kind(Kind::Texture).count();
                // EVERY riggable mesh, not only an unambiguous one: a weapon set holds four or five
                // pieces and an outfit folder holds tops/pants/gloves/shoes, so the editor OFFERS
                // the choice (the Load picker) instead of refusing the folder. The first is
                // pre-selected so the wizard is never stuck.
                let candidates: Vec<PathBuf> = scan.candidates().map(|e| e.path.clone()).collect();
                let error = candidates
                    .is_empty()
                    .then(|| format!("No riggable mesh in {}", dir.display()));
                let fbx = candidates.first().cloned().unwrap_or_default();
                tracing::info!(
                    "scanned {}: {} entries, {} riggable, {textures} textures",
                    dir.display(),
                    scan.entries.len(),
                    scan.riggable.len()
                );
                let ok = error.is_none();
                self.source = Some(Source {
                    dir,
                    scan,
                    fbx,
                    candidates,
                    candidate_sel: 0,
                    pick_window: 0,
                    textures,
                    parsed: None,
                    report: None,
                    // The class is DECLARED on the Task page, not guessed here — stamped from the
                    // workflow the user chose so the whole flow is intent-driven, not auto-detected.
                    class: self.pending_class,
                    // The sub-type is DECLARED on the Import panel (Accessory → garment/worn, Prop →
                    // static) exactly like the class — not guessed. `None` keeps the historical default.
                    prop: self.pending_prop.unwrap_or(PropKind::Accessory),
                    rig: None,
                    attach: Self::new_attach(),
                    attach_sel: 0,
                    fit: PropFit::default(),
                    fit_window: 0,
                    committed: None,
                    error,
                });
                // The DISPATCH: the class declared on the Task page picks WHICH linear
                // workflow definition runs (character rail vs the attach-less prop rail) —
                // then land DIRECTLY on the rig-edit view, single- or multi-mesh alike,
                // with zero extra clicks: the fresh workflow advances past Task through its
                // own gate (`conform` needs only the open `source`, which now exists). A
                // scan error lands there too, surfaced as the inspector's "Blocked:" line
                // rather than a dead Load page. When the folder holds several riggable
                // meshes the rig stage shows the inline piece picker (`on_pick`); the
                // first is pre-selected so the view is never empty.
                self.dispatch_workflow(self.pending_class);
                self.wf_advance();
                if ok {
                    self.analyze();
                    // conform() is idempotent and early-returns for Prop/Animation, so a model gets
                    // its rig here and a prop/animation simply lands on its own Conform role page
                    // (Mount / Clips) — the source-generic dispatch.
                    self.conform();
                }
            }
            Err(e) => tracing::error!("scan failed: {e}"),
        }
    }

    /// ANALYZE — parse the chosen FBX and measure it. Synchronous today, so a large
    /// source hitches one frame; folding the stages onto `flicker-worker::WorkerPool` is
    /// FDD Layer B and deliberately not started here.
    fn analyze(&mut self) {
        let Some(src) = self.source.as_mut() else { return };
        if src.parsed.is_some() || src.fbx.as_os_str().is_empty() {
            return;
        }
        match parse_fbx(&src.fbx) {
            Ok(model) => {
                tracing::info!(
                    "parsed {}: {} bones, {} verts",
                    src.fbx.display(),
                    model.bones.len(),
                    model.vertices.len()
                );
                let parsed = Parsed::new(model);
                // Classify sharpens the moment the skeleton is known — the bone count is its
                // deciding signal, so it is derived here rather than re-guessed per frame.
                let report = classify_asset(&src.scan, Some(parsed.bones()));
                // Seed the fit's STARTING socket from what was detected, so the Attach stage opens
                // on a sensible mount the user then confirms or moves — a weapon at the hand, a
                // garment at its body region, an accessory at the chest.
                let name = src.dir.file_name().and_then(|s| s.to_str()).unwrap_or("").to_string();
                let start = match report.class {
                    AssetClass::Prop if report.prop == PropKind::Clothing => garment_socket(&name),
                    AssetClass::Prop if report.prop == PropKind::Weapon => "hand_r",
                    AssetClass::Prop => "spine_02",
                    _ => "hand_r",
                };
                src.fit.socket = SOCKETS.iter().position(|(id, _)| *id == start).unwrap_or(0);
                src.report = Some(report);
                src.parsed = Some(parsed);
                src.error = None;
            }
            Err(e) => src.error = Some(format!("Parse failed: {e}")),
        }
    }

    /// CONFORM — rename to canonical names, then run the full conform against the reference rig,
    /// and read the per-bone provenance straight out of the reports. Runs once when the stage is
    /// reached; the sliders then author on top of its result.
    fn conform(&mut self) {
        let Some(src) = self.source.as_mut() else { return };
        if src.rig.is_some() {
            return;
        }
        // Conform is the CHARACTER path — it maps a biped skeleton onto the 66-bone
        // reference. A Prop or Animation has no such skeleton, so the wizard routes by the
        // confirmed class rather than forcing every asset through it: running it on a
        // skeleton-less mesh is exactly the misleading "no skeleton" failure. Their bake
        // paths (prop fit / clip retarget) are not wired in-app yet, and the stage says so.
        // (An unclassified asset falls through to the character path, as it always has.)
        if matches!(src.class(), Some(AssetClass::Prop | AssetClass::Animation)) {
            return;
        }
        let Some(parsed) = src.parsed.as_mut() else { return };
        let rename = rename_to_canonical(&mut parsed.model);
        match conform_to_canonical(&mut parsed.model, &default_reference()) {
            Ok(out) => {
                let map = bone_map_states(&parsed.model, &out);
                let n = parsed.model.bones.len();
                parsed.rebuild(&[]);
                tracing::info!(
                    "conformed {}: {} bones, {} inferred, {} renamed, {} unmapped",
                    src.fbx.display(),
                    n,
                    out.infer.added.len(),
                    rename.renamed,
                    rename.unmapped.len()
                );
                src.rig = Some(Rig {
                    rename,
                    out,
                    map,
                    offsets: vec![BoneOffset::default(); n],
                    sel: 0,
                    focused: false,
                    window: 0,
                });
                // The bones now carry canonical names, so the attach points can bind to them.
                src.resolve_attach();
                src.error = None;
            }
            Err(e) => src.error = Some(format!("Conform failed: {e}")),
        }
    }

    /// COMMIT — bake the conformed model and write `flicker.rig` into STAGING. The authored bone
    /// offsets are baked in by re-deriving the model first, so what eventually ships is exactly
    /// what the viewport showed.
    ///
    /// This writes the bench's OUTPUT; it does not publish. The asset reaches the tree the game
    /// loads from only when the Content Manager promotes it out of staging.
    fn commit(&mut self) {
        self.commit_to(&characters_dir());
    }

    /// The commit itself, against an explicit root — so the write path is exercisable against a
    /// scratch directory instead of the engine's live content tree. Dispatches by CLASS: a Skin
    /// bakes the conformed character (offsets applied), a Prop bakes a static mesh, a clothing Prop
    /// bakes a garment SKINNED onto the base body. `flicker-content` owns every bake; this only
    /// routes and records the outcome.
    fn commit_to(&mut self, root: &Path) {
        // Read everything under a shared borrow, then drop it before the write + the mutable
        // outcome record (so the borrow checker stays happy across the class dispatch).
        let (class, prop, name, model, has_rig, fit, fbx) = {
            let Some(src) = self.source.as_ref() else { return };
            let Some(parsed) = src.parsed.as_ref() else { return };
            let mut model = parsed.model.clone();
            // Only the character path has authored offsets to bake in; a prop/garment has none.
            if matches!(src.class(), Some(AssetClass::Skin) | None) {
                if let Some(rig) = src.rig.as_ref() {
                    apply_offsets(&mut model, &rig.offsets);
                }
            }
            // The human-authored placement the Attach stage tuned — what Commit bakes in.
            let fit = Fit {
                socket: src.fit.socket_name().to_string(),
                offset: src.fit.offset,
                rot_deg: src.fit.rot,
                scale: src.fit.scale,
                uniform: src.fit.uniform,
            };
            // The mesh file this came from — the prop/garment bakes read its FOLDER for the vendor's
            // texture maps, and its NAME tells one set piece's maps from another's.
            (src.class(), src.prop, src.asset_name().to_string(), model, src.rig.is_some(), fit, src.fbx.clone())
        };

        let dir = root.join(&name);
        let out = dir.join(format!("{name}.json"));
        let result: std::result::Result<(), String> = std::fs::create_dir_all(&dir)
            .map_err(|e| format!("Could not create {}: {e}", dir.display()))
            .and_then(|()| match class {
                // Clothing is a garment: a mesh SKINNED onto the base, its fit baked into the verts.
                Some(AssetClass::Prop) if prop == PropKind::Clothing => {
                    write_garment(&model, &fbx, &name, &out, &fit).map_err(|e| e.to_string())
                }
                // Any other prop is a rigid static mesh; the authored fit is written into its attach.
                Some(AssetClass::Prop) => {
                    write_prop(&model, &fbx, &name, &out, &fit).map_err(|e| e.to_string())
                }
                Some(AssetClass::Animation) => {
                    Err("Animation bake (clip retarget) is not wired into the editor yet.".to_string())
                }
                // Character: requires the conform to have produced a rig.
                _ => {
                    if has_rig {
                        write_rig(&model, &fbx, &name, &out).map_err(|e| e.to_string())
                    } else {
                        Err("Conform has not run — nothing to commit.".to_string())
                    }
                }
            });

        let Some(src) = self.source.as_mut() else { return };
        match result {
            Ok(()) => {
                tracing::info!("committed {}", out.display());
                src.committed = Some(out);
                src.error = None;
            }
            Err(e) => src.error = Some(e),
        }
    }

    /// The engine-requirement checks the Review stage reports — each computed from real state, so
    /// a red line is a real blocker and not a placeholder.
    fn requirements(&self) -> Vec<(bool, String)> {
        let Some(src) = self.source.as_ref() else { return Vec::new() };
        let verts = src.parsed.as_ref().map(|p| p.verts).unwrap_or(0);
        // Prop / garment / animation carry their OWN requirement set — the character skeleton/attach
        // checks below do not apply to them.
        // Every requirement's static copy is a `$token`; the live counts compose around
        // the resolved text (the sanctioned composed-string shape).
        let r = |t: &str| strings::resolve(t).into_owned();
        match src.class() {
            Some(AssetClass::Prop) if src.prop == PropKind::Clothing => {
                return vec![
                    (
                        verts > 0,
                        format!("{} ({verts} {})", r("$ap_req_garment_mesh_present"), r("$ap_verts")),
                    ),
                    (true, r("$ap_req_skins_onto_the_canonical_base")),
                ];
            }
            Some(AssetClass::Prop) => {
                return vec![
                    (
                        verts > 0,
                        format!("{} ({verts} {})", r("$ap_req_prop_mesh_present"), r("$ap_verts")),
                    ),
                    (true, r("$ap_req_socket_fit_authored_in_the_paperdoll")),
                ];
            }
            Some(AssetClass::Animation) => {
                return vec![(false, r("$ap_req_animation_bake_is_not_wired_yet"))];
            }
            _ => {}
        }
        // Character (Skin / unclassified): reported in BAKED terms — the +1 is the root `bake_rig`
        // synthesizes — so the figure here is the one the shipped rig will carry.
        let conformed = src.parsed.as_ref().map(|p| p.bones()).unwrap_or(0);
        let baked = if conformed == 0 { 0 } else { conformed + 1 };
        let mut out = vec![(
            conformed == CONFORMED_BONES,
            format!("{} ({baked} / {REFERENCE_BONES} {})", r("$ap_req_skeleton_conforms"), r("$ap_bones")),
        )];
        match src.rig.as_ref() {
            None => out.push((false, r("$ap_req_conform_has_not_run"))),
            Some(rig) => {
                let (_, review, _) = rig.counts();
                out.push((
                    rig.rename.unmapped.is_empty(),
                    if rig.rename.unmapped.is_empty() {
                        format!(
                            "{} ({review} {})",
                            r("$ap_req_all_bones_mapped_or_reviewed"),
                            r("$ap_flagged")
                        )
                    } else {
                        format!("{} {}", rig.rename.unmapped.len(), r("$ap_req_source_bones_unmapped"))
                    },
                ));
            }
        }
        let resolved = (0..src.attach.len()).filter(|i| self.attach_resolved(*i)).count();
        out.push((
            resolved == src.attach.len(),
            format!("{} ({resolved} / {})", r("$ap_req_attach_points_on_valid_parents"), src.attach.len()),
        ));
        out.push((
            src.textures > 0,
            format!("{} ({} {})", r("$ap_req_textures_masks_resolved"), src.textures, r("$ap_found")),
        ));
        out
    }

    /// Whether an attach point's parent bone exists in the conformed rig.
    fn attach_resolved(&self, i: usize) -> bool {
        self.source
            .as_ref()
            .and_then(|s| s.attach.get(i))
            .is_some_and(|p| p.bone.is_some())
    }

    /// Upload (and cache) the loaded source model's OWN mesh, textured with its source maps. THE one
    /// "imported geometry on the GPU" path — shared by the CHARACTER preview (the body being rigged)
    /// and the PROP/GARMENT preview (the piece being fitted). The class decides how the mesh is
    /// PLACED, never how it is built — so any folder `parse_fbx` reads previews the same way, with no
    /// per-model special-casing. Keyed by model identity, so re-picking a candidate or turning the
    /// orientation control re-uploads (freeing the previous upload first). `None` before a mesh
    /// exists. Must run before `grid.render` borrows the renderer for the RTT passes.
    fn ensure_source_mesh(&mut self, r: &mut Renderer) -> Option<Uploaded> {
        let (key, has_mesh) = {
            let src = self.source.as_ref()?;
            let has_mesh = src.parsed.as_ref().map(|p| !p.model.vertices.is_empty()).unwrap_or(false);
            let key: PreviewKey = (src.dir.clone(), src.candidate_sel);
            (key, has_mesh)
        };
        if !has_mesh {
            return None;
        }
        let need = match &self.preview {
            Some((_, k)) => *k != key,
            None => true,
        };
        if need {
            if let Some((old, _)) = self.preview.take() {
                old.free(r);
            }
            // Own the geometry + the map paths first, so the immutable borrow of `source` ends
            // before the texture cache is borrowed mutably for the upload.
            let (verts, uvs, indices, maps) = {
                let src = self.source.as_ref()?;
                let parsed = src.parsed.as_ref()?;
                let verts: Vec<MeshVertex> = parsed
                    .model
                    .vertices
                    .iter()
                    .map(|v| MeshVertex { position: v.p, normal: v.n, material: 0 })
                    .collect();
                let uvs: Vec<[f32; 2]> = parsed.model.vertices.iter().map(|v| v.uv).collect();
                // The SAME classifier the BAKE uses (`wire_textures` calls it too), so the mesh
                // previews with exactly the map set Commit will write beside its rig — one rule
                // for "which PNG is the albedo", not one for the viewport and one for disk.
                let maps = source_maps(&src.scan, &src.fbx);
                (verts, uvs, parsed.model.indices.clone(), maps)
            };
            let up = upload_preview(r, &mut self.textures, &maps, &verts, &uvs, &indices);
            self.preview = Some((up, key));
        }
        self.preview.as_ref().map(|(h, _)| *h)
    }

    /// The LIVE CPU-skinned character mesh, for the Conform stage: skins `parsed.model.vertices`
    /// through the posed `parsed.globals` (the authored offsets are already folded into them) and
    /// re-uploads whenever the pose changes. At the conform rest pose the palette is the identity, so
    /// this reproduces the bind mesh exactly; an authored offset deforms it — the live re-skin the
    /// gizmo drives. Same upload path (and texture cache) as [`Self::ensure_source_mesh`], so a
    /// textured source previews with its real maps. `None` when there is nothing skinnable.
    fn ensure_skinned_mesh(&mut self, r: &mut Renderer) -> Option<Uploaded> {
        let (key, ok) = {
            let src = self.source.as_ref()?;
            let parsed = src.parsed.as_ref()?;
            let ok = !parsed.model.vertices.is_empty() && !parsed.model.bones.is_empty();
            let key = ((src.dir.clone(), src.candidate_sel), self.pose_gen);
            (key, ok)
        };
        if !ok {
            return None;
        }
        let need = match &self.skinned {
            Some((_, k)) => *k != key,
            None => true,
        };
        if need {
            if let Some((old, _)) = self.skinned.take() {
                old.free(r);
            }
            // Own the geometry + map paths first, so the immutable borrow of `source` ends before
            // the texture cache is borrowed mutably for the upload (same ordering as the rest mesh).
            let (verts, uvs, indices, maps) = {
                let src = self.source.as_ref()?;
                let parsed = src.parsed.as_ref()?;
                let verts = skin_source_verts(&parsed.model, &parsed.globals);
                let uvs: Vec<[f32; 2]> = parsed.model.vertices.iter().map(|v| v.uv).collect();
                let maps = source_maps(&src.scan, &src.fbx);
                (verts, uvs, parsed.model.indices.clone(), maps)
            };
            let up = upload_preview(r, &mut self.textures, &maps, &verts, &uvs, &indices);
            self.skinned = Some((up, key));
        }
        self.skinned.as_ref().map(|(h, _)| *h)
    }

    /// The pick ray in the quad panel UNDER the cursor, in the SAME recentred draw space the skeleton
    /// is drawn in, plus WHICH cell it hit — so the caller can use the Perspective arrows (cell 0) or
    /// an ortho panel's view plane (cells 1–3). Built from that cell's own camera exactly as
    /// [`Self::render`] frames it (view 0 has no ortho, so its camera is the perspective orbit
    /// verbatim). `None` when the cursor is outside the grid.
    fn pick_ray_at(&self, cursor: Vec2, screen: Vec2) -> Option<(usize, (Vec3, Vec3))> {
        let grid = self.grid.as_ref()?;
        let cell = grid.cell_at(cursor, screen)?;
        let local = grid.local_cursor(cell, cursor, screen)?;
        let vp = grid.cell(cell, screen).size;
        let o = &self.orbits[cell];
        let cam = grid.camera(cell, o.ortho_radius(self.view_radius), &o.camera(self.view_radius));
        cam.pick_ray(local, vp).map(|ray| (cell, ray))
    }

    /// Publish the gizmo-mode toggle captions (the radio glyph + label) — set in BOTH conform-model
    /// branches so the rig-less default model (the HUD-walk test) still renders real captions.
    fn set_gizmo_modes(&self, m: &mut ValueMap) {
        m.set(
            "mode_translate",
            format!("{} {}", radio(self.gizmo_mode == GizmoMode::Translate), strings::resolve("$ap_translate")),
        );
        m.set(
            "mode_rotate",
            format!("{} {}", radio(self.gizmo_mode == GizmoMode::Rotate), strings::resolve("$ap_rotate")),
        );
        m.set(
            "mode_scale",
            format!("{} {}", radio(self.gizmo_mode == GizmoMode::Scale), strings::resolve("$ap_scale_gizmo")),
        );
    }

    /// Set (or clear) the FOCUSED joint — the one carrying the gizmo. Focus follows selection, so the
    /// bone-map row tracks it; `None` deselects (clears the gizmo; ortho nudges then do nothing).
    fn set_focus(&mut self, sel: Option<usize>) {
        if let Some(rig) = self.source.as_mut().and_then(|s| s.rig.as_mut()) {
            match sel {
                Some(i) => {
                    rig.sel = i;
                    rig.focused = true;
                    Self::scroll_to_selection(rig);
                }
                None => rig.focused = false,
            }
        }
    }

    /// The in-scene joint-editing interaction (Conform stage), keyed off FOCUS. PERSPECTIVE (cell 0):
    /// click a joint to FOCUS it (empty space deselects), or drag its arrow to DEFORM (test the skin).
    /// ORTHO panels (Top/Side/Front): a plain click-drag moves the FOCUSED joint in that view's plane
    /// (repositioning the rest skeleton, mesh static); Ctrl-click CHANGES focus (or deselects on a
    /// miss) — so nudging never grabs a neighbour. Returns whether it handled the pointer, so the
    /// caller suppresses the orbit on the same gesture.
    fn gizmo_interact(&mut self, input: &InputState, screen: Vec2) -> bool {
        if self.wf.step() != "conform" {
            self.gizmo_drag = None;
            return false;
        }
        let Some((cell, (o, d))) = self.pick_ray_at(input.mouse_position, screen) else {
            // Cursor left the grid: drop a drag on release, otherwise hold it for next frame.
            if !input.mouse_left {
                self.gizmo_drag = None;
            }
            return self.gizmo_drag.is_some();
        };
        // Owned reads, so the `source` borrow ends before any write. `globals` is the pre-write pose,
        // which is exactly what the world→parent-local conversion needs.
        let Some((centre, radius, globals, sel, focused)) = self
            .source
            .as_ref()
            .and_then(|s| s.parsed.as_ref().zip(s.rig.as_ref()))
            .map(|(p, r)| (p.centre, p.radius, p.globals.clone(), r.sel, r.focused))
        else {
            return false;
        };
        let recentre = Mat4::from_translation(-centre);
        let joint_tol = (radius * GIZMO_ARROW_FRAC).max(1.0) * 0.35;
        let origin =
            recentre.transform_point3(globals.get(sel).map(|g| g.w_axis.truncate()).unwrap_or(Vec3::ZERO));
        // Bespoke raw key, per the new input system's "non-ActionSignal keys read raw per scene".
        let ctrl = input.key_down(Key::LeftControl) || input.key_down(Key::RightControl);

        // Continue an active drag. Perspective = a Deform TEST (springs back on release); ortho =
        // a Reposition (permanent). Both drag in the view plane.
        if let Some((mode, ray_prev)) = self.gizmo_drag {
            if !input.mouse_left {
                // Release: a perspective deform test snaps the joint back to its pre-drag rest.
                if let DragMode::Deform { restore, .. } = mode {
                    self.restore_offset(sel, restore);
                }
                self.gizmo_drag = None;
                return false;
            }
            let normal = match mode {
                DragMode::Deform { normal, .. } => normal,
                DragMode::Reposition { normal } => normal,
            };
            let world_delta = drag_plane(normal, origin, ray_prev, (o, d));
            self.gizmo_drag = Some((mode, (o, d)));
            if world_delta != Vec3::ZERO {
                match mode {
                    // Perspective: DEFORM the mesh to preview the skin (ephemeral; springs back).
                    DragMode::Deform { .. } => self.apply_gizmo_delta(sel, &globals, world_delta),
                    // Ortho: REPOSITION the rest skeleton, mesh static (then Bake Skin).
                    DragMode::Reposition { .. } => self.reposition_bone(sel, &globals, world_delta),
                }
            }
            return true;
        }

        if !input.mouse_left_pressed {
            return false;
        }

        // The joint ball nearest the pick ray (screen-nearest in an ortho view), if within tolerance.
        let nearest = || -> Option<usize> {
            let mut best: Option<(usize, f32)> = None;
            for (i, g) in globals.iter().enumerate() {
                let c = recentre.transform_point3(g.w_axis.truncate());
                let (pr, ps) = closest_point_ray_segment(o, d, c, c);
                let miss = (pr - ps).length();
                if best.is_none_or(|(_, bd)| miss < bd) {
                    best = Some((i, miss));
                }
            }
            best.filter(|&(_, miss)| miss <= joint_tol).map(|(i, _)| i)
        };

        if cell == 0 {
            // PERSPECTIVE — grab a joint to FOCUS it and free-drag it as a spring-back DEFORM TEST:
            // the mesh deforms while you hold, and on release the joint snaps back to rest. Empty
            // space DESELECTS and leaves the orbit free.
            match nearest() {
                Some(i) => {
                    let restore = self
                        .source
                        .as_ref()
                        .and_then(|s| s.rig.as_ref())
                        .and_then(|r| r.offsets.get(i).copied())
                        .unwrap_or_default();
                    self.set_focus(Some(i));
                    self.gizmo_drag =
                        Some((DragMode::Deform { normal: d.normalize_or_zero(), restore }, (o, d)));
                    true
                }
                None => {
                    self.set_focus(None);
                    false
                }
            }
        } else if ctrl {
            // ORTHO + Ctrl — the deliberate "change focus" gesture: nearest joint focuses, a miss
            // deselects. Without Ctrl an ortho click never re-selects, so a nudge of the focused joint
            // can't accidentally grab a neighbour.
            match nearest() {
                Some(i) => self.set_focus(Some(i)),
                None => self.set_focus(None),
            }
            true
        } else if focused {
            // ORTHO, no modifier — the click belongs to the FOCUSED joint: begin a Reposition drag in
            // this view's plane (normal = the view direction `d`), moving it in the two on-screen axes.
            self.gizmo_drag = Some((DragMode::Reposition { normal: d.normalize_or_zero() }, (o, d)));
            true
        } else {
            false
        }
    }

    /// Write a WORLD-space translation delta onto the selected bone's [`BoneOffset::t`], converting
    /// it to the bone's PARENT-local frame first — that is the space `t` lives in (folded into the
    /// pose by [`rest_globals`], so the sliders and the gizmo share one value). `recentre` is
    /// translation-only, so the world delta needs no un-recentring. Re-derives the pose and bumps
    /// `pose_gen` so the mesh re-skins live.
    fn apply_gizmo_delta(&mut self, sel: usize, globals: &[Mat4], world_delta: Vec3) {
        // Parent's current posed rotation/scale takes the world delta into parent-local (root = world).
        let parent_inv = self
            .source
            .as_ref()
            .and_then(|s| s.parsed.as_ref())
            .and_then(|p| p.model.bones.get(sel))
            .and_then(|b| usize::try_from(b.parent).ok())
            .and_then(|pi| globals.get(pi))
            .map(|g| glam::Mat3::from_mat4(*g).inverse())
            .unwrap_or(glam::Mat3::IDENTITY);
        let dt = parent_inv * world_delta;
        if !dt.is_finite() {
            return;
        }
        let Some(src) = self.source.as_mut() else { return };
        let Some(slot) = src.rig.as_mut().and_then(|r| r.offsets.get_mut(sel)) else { return };
        slot.t[0] += dt.x;
        slot.t[1] += dt.y;
        slot.t[2] += dt.z;
        let offsets = src.rig.as_ref().map(|r| r.offsets.clone()).unwrap_or_default();
        if let Some(parsed) = src.parsed.as_mut() {
            parsed.rebuild(&offsets);
        }
        self.pose_gen = self.pose_gen.wrapping_add(1);
    }

    /// ORTHO reposition (the tool's core): move the bone's REST position and RE-BIND, so only the
    /// SKELETON moves — the mesh stays put. Unlike [`apply_gizmo_delta`] (Perspective, which DEFORMS
    /// to preview the skin), this edits the rest local translation and re-derives every `inverse_bind`
    /// to the new rest, keeping the palette identity at rest. Mirrors to the symmetric `_l`/`_r` joint
    /// when `mirror_joints` is on. Bake Skin afterwards to re-weight to the corrected skeleton.
    fn reposition_bone(&mut self, sel: usize, globals: &[Mat4], world_delta: Vec3) {
        let mirror = self.mirror_joints.then(|| self.mirror_of(sel)).flatten();
        let offsets = self
            .source
            .as_ref()
            .and_then(|s| s.rig.as_ref())
            .map(|r| r.offsets.clone())
            .unwrap_or_default();
        let Some(p) = self.source.as_mut().and_then(|s| s.parsed.as_mut()) else {
            return;
        };
        let dt = parent_local_delta(globals, &p.model, sel, world_delta);
        if !dt.is_finite() {
            return;
        }
        for k in 0..3 {
            p.model.bones[sel].translation[k] += dt[k];
        }
        if let Some(m) = mirror {
            // Mirror across the body's symmetry plane (world X): negate the X of the move.
            let mworld = Vec3::new(-world_delta.x, world_delta.y, world_delta.z);
            let mdt = parent_local_delta(globals, &p.model, m, mworld);
            if mdt.is_finite() {
                for k in 0..3 {
                    p.model.bones[m].translation[k] += mdt[k];
                }
            }
        }
        // Re-bind: inverse_bind = (new rest world)⁻¹ for every bone → the mesh is unchanged at rest;
        // only the skeleton has moved. Bake Skin then re-weights to the corrected bones.
        let (rest, _) = rest_globals(&p.model, &[]);
        for (b, g) in p.model.bones.iter_mut().zip(&rest) {
            b.inverse_bind = g.inverse().to_cols_array();
        }
        p.rebuild(&offsets);
        self.pose_gen = self.pose_gen.wrapping_add(1);
        // A PERMANENT conform edit — arm the workflow's Back discard guard, exactly as a
        // slider write does (the Perspective deform TEST springs back, so it never arms it).
        self.wf.set_dirty(true);
    }

    /// The symmetric bone of `sel` (its `_l`/`_r` twin), if the rig has one.
    fn mirror_of(&self, sel: usize) -> Option<usize> {
        let p = self.source.as_ref()?.parsed.as_ref()?;
        let mname = mirror_name(&p.model.bones.get(sel)?.name)?;
        p.model.bones.iter().position(|b| b.name == mname)
    }

    /// Re-bake the character's skin WEIGHTS from the repositioned skeleton (replacing the source's
    /// auto-skin), then re-skin the view. The rest pose is untouched, so the mesh does not move — only
    /// its deformation changes, which the Perspective panel shows.
    fn bake_skin_now(&mut self) {
        let offsets = self
            .source
            .as_ref()
            .and_then(|s| s.rig.as_ref())
            .map(|r| r.offsets.clone())
            .unwrap_or_default();
        if let Some(p) = self.source.as_mut().and_then(|s| s.parsed.as_mut()) {
            bake_skin(&mut p.model);
            p.rebuild(&offsets);
        }
        self.pose_gen = self.pose_gen.wrapping_add(1);
        // Replacing the skin weights is a conform edit Next would commit — guard Back.
        self.wf.set_dirty(true);
    }

    /// Restore the focused joint's [`BoneOffset`] to `offset` (its pre-drag value) — the spring-back
    /// that ends a Perspective deform TEST, snapping the joint back to its rest position.
    fn restore_offset(&mut self, sel: usize, offset: BoneOffset) {
        let Some(src) = self.source.as_mut() else { return };
        if let Some(slot) = src.rig.as_mut().and_then(|r| r.offsets.get_mut(sel)) {
            *slot = offset;
        }
        let offsets = src.rig.as_ref().map(|r| r.offsets.clone()).unwrap_or_default();
        if let Some(p) = src.parsed.as_mut() {
            p.rebuild(&offsets);
        }
        self.pose_gen = self.pose_gen.wrapping_add(1);
    }

    /// Build this frame's PROP/GARMENT fit preview — the reference body plus the imported mesh
    /// placed at `socket · fit` through the SAME `attach_world` the bake uses. `None` for a
    /// character (whose own mesh the main path draws directly via [`Self::ensure_source_mesh`]) or
    /// before a mesh exists. Called first in `render` so the upload has `&mut Renderer` to itself.
    fn ensure_preview(&mut self, r: &mut Renderer) -> Option<PreviewDraw> {
        let (fit, is_char) = {
            let src = self.source.as_ref()?;
            (src.fit, matches!(src.class(), Some(AssetClass::Skin) | None))
        };
        if is_char {
            return None;
        }
        let mesh = self.ensure_source_mesh(r)?;
        if self.base.is_none() {
            self.base = BasePreview::load();
        }

        let base = self.base.as_ref()?;
        let socket_name = SOCKETS.get(fit.socket).map(|(id, _)| *id).unwrap_or("pelvis");
        // Place the mesh at the socket's rest frame · the authored fit — identical math to the bake.
        let world = base
            .socket_index(socket_name)
            .map(|i| {
                let f = Fit {
                    socket: socket_name.to_string(),
                    offset: fit.offset,
                    rot_deg: fit.rot,
                    scale: fit.scale,
                    uniform: fit.uniform,
                };
                attach_world(&base.ibind[i], &f.to_attach())
            })
            .unwrap_or(Mat4::IDENTITY);
        // Centre the body — and the piece with it — because the quad cameras target the ORIGIN,
        // which in Z-up ground reckoning is the body's feet.
        let recentre = Mat4::from_translation(-base.centre);
        let base_joints = debug::bone_diamonds(recentre, &base.parents, &base.globals, BONE_WAIST_FRAC);
        Some(PreviewDraw {
            mesh,
            world: recentre * world,
            base_joints,
            radius: base.radius,
            floor: base.floor,
            base_mesh: self.show_base.then_some(self.base_upload).flatten(),
            base_world: recentre,
        })
    }

    /// The values the HUD binds against. Rust owns ALL formatting — the walker has no
    /// printf — so every readout is a pre-built string here. `&mut` because the workflow
    /// runtime's own [`Workflow::publish`] rides the same Model (surfaces, rail chips,
    /// discard dialog, footer step keys) — one publish, one frame, no split state.
    /// The bench's UI tree for this frame: a screen root that DECLARES its input as
    /// data, one `clayworks_bench` template instance, and [`expand`] as the single
    /// seam. Rebuilt every frame — re-expansion is what keeps template params live.
    ///
    /// Controller is the floor (BA4487BD), using only RATIFIED signal names:
    /// A/B are the wizard's forward/back, and the bumpers walk the same rail because
    /// the step strip IS this screen's tab bar. `Mode*` cycles the gizmo on the rig
    /// page — the one genuine second axis the bench has. Nothing is declared that the
    /// dispatcher below does not act on: a bound signal with no arm is dead hardware.
    pub fn build_tree(&self, _screen: Vec2) -> UiNode {
        let mut page = UiNode { component: "screen".into(), id: "assetpipeline".into(), ..Default::default() };
        for (signal, result) in [
            ("on_menu", "pause_open"),
            ("on_confirm", "wf_next"),
            ("on_cancel", "wf_back"),
            ("on_tab_next", "wf_next"),
            ("on_tab_prev", "wf_back"),
            ("on_mode_next", "gizmo_next"),
            ("on_mode_prev", "gizmo_prev"),
        ] {
            page.props.insert(signal.into(), Value::Text(result.into()));
        }
        page.children =
            vec![UiNode { template: Some(BENCH_TEMPLATE.into()), ..Default::default() }];
        // Expanded HERE, not at the call sites, so the scene and every gate walk the
        // SAME tree — a proto left unresolved would otherwise draw a bare box in the
        // app while the tests inspected a `template` node they never opened (8634C200).
        expand(page, &self.templates)
    }

    /// The gizmo-mode toggle's dispatcher — POINTER and PAD reaching one piece of
    /// state by two routes. The three `mode_*` buttons are the exclusive selection the
    /// inspector draws; `gizmo_next` / `gizmo_prev` are the names the screen's
    /// `on_mode_next` / `on_mode_prev` declarations fire, and they CYCLE (with wrap) so
    /// one controller axis covers all three modes without a button per mode.
    ///
    /// Split out of `update` so the declared-intent gate can drive it directly: an
    /// intent whose arm lives inline in the frame loop can only be tested by running a
    /// frame, and a gate that cannot run is a gate that stops being written.
    fn apply_gizmo_results(&mut self, results: &ValueMap) {
        if results.is_on("mode_translate") {
            self.gizmo_mode = GizmoMode::Translate;
        }
        if results.is_on("mode_rotate") {
            self.gizmo_mode = GizmoMode::Rotate;
        }
        if results.is_on("mode_scale") {
            self.gizmo_mode = GizmoMode::Scale;
        }
        if results.is_on("gizmo_next") {
            self.gizmo_mode = match self.gizmo_mode {
                GizmoMode::Translate => GizmoMode::Rotate,
                GizmoMode::Rotate => GizmoMode::Scale,
                GizmoMode::Scale => GizmoMode::Translate,
            };
        }
        if results.is_on("gizmo_prev") {
            self.gizmo_mode = match self.gizmo_mode {
                GizmoMode::Translate => GizmoMode::Scale,
                GizmoMode::Rotate => GizmoMode::Translate,
                GizmoMode::Scale => GizmoMode::Rotate,
            };
        }
    }

    fn hud_model(&mut self) -> ValueMap {
        let mut m = ValueMap::default();
        let step = self.wf.step().to_string();
        // Conform's title / hint come from its ROLE, so a prop never reads "Conform Rig"
        // over a mount page. Footer copy is `$token`s resolved here (the Model-channel
        // strings gate), like the rail labels riding `$wf_step_*` via `publish` below.
        let role = self.conform_role();
        let (title, hint) = match step.as_str() {
            "conform" => (role.title(), role.hint()),
            "attach" => ("$ap_attach_points", "$ap_position_hold_holster_and_belt_attach_po"),
            "review" => ("$ap_review_export", "$ap_verify_engine_requirements_then_hand_off"),
            // "task" — and the unreachable arm a bad definition would land in.
            _ => ("$ap_select_workflow", "$ap_choose_the_kind_of_asset_you_are_importi"),
        };
        m.set("step_title", strings::resolve(title).into_owned());
        m.set("step_hint", strings::resolve(hint).into_owned());
        m.set("show_skeleton", self.show_skeleton);
        m.set("show_base", self.show_base);
        m.set("show_collision", self.show_collision);
        m.set("mirror", self.mirror_joints);

        // Rail chips are ENTIRELY the workflow runtime's: `publish` below writes each
        // step's `wf_<id>_title` (pre-localized), `wf_<id>_style` (workflow.chip.active /
        // visited / todo) AND `wf_<id>_show` membership — so the rail derives from the
        // running definition (the prop definition has no `attach`, so its chip's bind is
        // simply absent and the slot leaves the row's grow layout). Nothing to publish
        // here; the chips stay non-interactive and the footer moves the step.

        let has = self.source.is_some();
        m.set("has_asset", has);
        match self.source.as_ref() {
            None => {
                m.set("asset_name", strings::resolve("$ap_no_asset_loaded").into_owned());
                m.set("asset_file", "");
            }
            Some(src) => {
                m.set("asset_name", src.asset_name());
                m.set("asset_file", src.file_name());
            }
        }

        // Inspector body: up to 8 pre-formatted lines, whatever this step genuinely knows.
        let lines = self.inspector_lines();
        for i in 0..INSPECTOR_LINES {
            m.set(format!("insp_{i}"), lines.get(i).cloned().unwrap_or_default());
        }
        m.set("insp_title", self.inspector_title());
        m.set("insp_badge", self.inspector_badge());
        // Back is live once a folder is open — on the Workflow page (the first stop) it CLEARS the
        // folder, the only meaningful "back" there (reported: Back did nothing on that page).
        m.set("back_enabled", step != "task" || self.source.is_some());
        // Forward gating is the runtime's document contract (`can_next` — the next step's
        // declared needs against this frame's document). The final (Review) stage's
        // forward button becomes "Restart" — always enabled, it loops back to the
        // Workflow page to process another asset (scene policy, not the runtime's).
        let doc = self.wf_doc();
        m.set("next_enabled", self.wf.can_next(&doc) || step == "review");
        // The caption rides the workflow proto's `next_label_bind` — published RESOLVED
        // through the stringtable so no raw English enters the Model.
        m.set(
            "next_label",
            strings::resolve(if step == "review" { "$wf_restart" } else { "$wf_next" }).into_owned(),
        );
        // The piece picker rides on the rig stage, and only when the folder holds a choice of meshes
        // (a weapon set, an outfit) — a single-mesh folder never shows it.
        let several = self.source.as_ref().map(|s| s.candidates.len() > 1).unwrap_or(false);
        m.set("on_pick", step == "conform" && several);
        self.pick_model(&mut m);

        // Per-step content gates by the step's SURFACE key ("task" / "attach" / "review"),
        // published by the runtime below — exactly one subtree is live. CONFORM is the one
        // step carrying SEVERAL ROLES, so its panels refine the step with the role (scene
        // logic — the runtime knows steps, not classes): a Skin gets the bone map and its
        // offsets; a prop/garment gets the mount socket + placement (its rig IS that
        // binding); an animation gets an honest "not wired" instead of controls
        // addressing nothing.
        let at_conform = step == "conform";
        m.set("on_conform_skeleton", at_conform && role == ConformRole::Skeleton);
        m.set("on_conform_mount", at_conform && role == ConformRole::Mount);
        m.set("on_conform_clip", at_conform && role == ConformRole::Clip);
        self.conform_model(&mut m);
        self.attach_model(&mut m);
        self.fit_model(&mut m);
        self.review_model(&mut m);
        // The workflow runtime's OWN publish: the step surfaces (exclusive on the current
        // step — the "task"/"attach"/"review" gates above), the `wf_discard` dialog, the
        // rail chips (`wf_<id>_title` / `wf_<id>_style`) and the footer step keys
        // (`wf_step`, `wf_step_i`/`_n`, `wf_can_next`).
        self.wf.publish(&doc, &mut m);
        // The transient `sig_<name>` mirror (S9): intent names fired last frame
        // ride exactly this ONE publish for scripts to observe (`update` clears
        // them right after the walk). Distinct from the step-surface MODEL binds
        // above — those are visibility gates, not root intent props.
        UiIntents::mirror_into(&mut m, &self.fired_sigs);
        m
    }

    /// CONFORM bindings: the mapped/total headline, the paged bone map, and the four offset
    /// sliders for the selected bone.
    fn conform_model(&self, m: &mut ValueMap) {
        let rig = self.source.as_ref().and_then(|s| s.rig.as_ref());
        let parsed = self.source.as_ref().and_then(|s| s.parsed.as_ref());
        let (Some(rig), Some(parsed)) = (rig, parsed) else {
            for i in 0..BONE_ROWS {
                m.set(format!("bone_{i}"), "");
                m.set(format!("bone_{i}_color"), MapState::Ok.color());
                m.set(format!("bone_{i}_on"), false);
            }
            m.set("rig_headline", strings::resolve("$ap_conform_has_not_run").into_owned());
            m.set("rig_legend", "");
            m.set("rig_sel", strings::resolve("$ap_no_bone_selected").into_owned());
            m.set("rig_progress", 0.0);
            for (k, _) in OFFSET_AXES {
                m.set(k, 0.0);
            }
            self.set_gizmo_modes(m);
            m.set("has_rig", false);
            return;
        };
        m.set("has_rig", true);
        let (ok, review, auto) = rig.counts();
        let total = rig.map.len();
        m.set(
            "rig_headline",
            format!("{}      {ok} / {total}", strings::resolve("$ap_source_internal_rig")),
        );
        m.set(
            "rig_legend",
            format!(
                "\u{25cf} {review} {}      \u{25cf} {auto} {}",
                strings::resolve("$ap_need_review"),
                strings::resolve("$ap_auto_inferred")
            ),
        );
        m.set("rig_progress", if total > 0 { ok as f64 / total as f64 } else { 0.0 });

        // The visible window of the bone map — six rows of a 66-row list.
        for i in 0..BONE_ROWS {
            let idx = rig.window + i;
            match (parsed.model.bones.get(idx), rig.map.get(idx)) {
                (Some(b), Some(state)) => {
                    let edited = rig.offsets.get(idx).is_some_and(|o| !o.is_zero());
                    m.set(
                        format!("bone_{i}"),
                        format!(
                            "{}  {:<22}{}{}",
                            if idx == rig.sel { "\u{25b8}" } else { " " },
                            b.name,
                            strings::resolve(state.tag()),
                            if edited { " *" } else { "" }
                        ),
                    );
                    m.set(format!("bone_{i}_color"), state.color());
                    m.set(format!("bone_{i}_on"), idx == rig.sel);
                }
                _ => {
                    m.set(format!("bone_{i}"), "");
                    m.set(format!("bone_{i}_color"), MapState::Ok.color());
                    m.set(format!("bone_{i}_on"), false);
                }
            }
        }
        m.set(
            "bone_page",
            format!(
                "{}\u{2013}{} {} {total}",
                rig.window + 1,
                (rig.window + BONE_ROWS).min(total),
                strings::resolve("$ap_of")
            ),
        );
        m.set("bone_prev_enabled", rig.sel > 0);
        m.set("bone_next_enabled", rig.sel + 1 < total);

        let name = parsed.model.bones.get(rig.sel).map(|b| b.name.as_str()).unwrap_or("—");
        m.set("rig_sel", format!("{name} \u{2192} {}", strings::resolve("$ap_offset")));
        let o = rig.offsets.get(rig.sel).copied().unwrap_or_default();
        for (i, (key, _)) in OFFSET_AXES.into_iter().enumerate() {
            m.set(key, if i < 3 { o.t[i] as f64 } else { o.roll as f64 });
        }
        self.set_gizmo_modes(m);
    }

    /// ATTACH bindings: the six points, their parent bones, and the selected point's offsets.
    fn attach_model(&self, m: &mut ValueMap) {
        let Some(src) = self.source.as_ref() else {
            for i in 0..ATTACH_POINTS.len() {
                m.set(format!("att_{i}"), "");
                m.set(format!("att_{i}_on"), false);
            }
            m.set("att_sel", strings::resolve("$ap_no_point_selected").into_owned());
            for (k, _) in ATTACH_AXES {
                m.set(k, 0.0);
            }
            return;
        };
        for (i, p) in src.attach.iter().enumerate() {
            let resolved = self.attach_resolved(i);
            m.set(
                format!("att_{i}"),
                format!(
                    "{} {:<18}{} {}{}",
                    if i == src.attach_sel { "\u{25c6}" } else { "\u{25c7}" },
                    strings::resolve(p.label),
                    strings::resolve("$ap_parent"),
                    p.parent,
                    if resolved {
                        String::new()
                    } else {
                        format!(" {}", strings::resolve("$ap_absent"))
                    }
                ),
            );
            m.set(format!("att_{i}_on"), i == src.attach_sel);
        }
        let sel = src.attach.get(src.attach_sel);
        m.set(
            "att_sel",
            strings::resolve(sel.map(|p| p.label).unwrap_or("$ap_no_point_selected")).into_owned(),
        );
        let off = sel.map(|p| p.offset).unwrap_or([0.0; 3]);
        for (i, (key, _)) in ATTACH_AXES.into_iter().enumerate() {
            m.set(key, off[i] as f64);
        }
    }

    /// LOAD-PICKER bindings: every riggable mesh in the opened folder, so a weapon set (four or five
    /// pieces) or an outfit folder (tops / pants / gloves / shoes) is imported piece by piece
    /// instead of being refused. Shown only when the folder actually holds a choice.
    fn pick_model(&self, m: &mut ValueMap) {
        let (cands, sel, window) = match self.source.as_ref() {
            Some(s) => (s.candidates.as_slice(), s.candidate_sel, s.pick_window),
            None => (&[][..], 0usize, 0usize),
        };
        for i in 0..PICK_ROWS {
            let idx = window + i;
            match cands.get(idx) {
                Some(p) => {
                    let on = idx == sel;
                    let name = p.file_name().and_then(|s| s.to_str()).unwrap_or("");
                    m.set(
                        format!("pick_{i}"),
                        format!("{}  {name}", if on { "\u{25c6}" } else { "\u{25c7}" }),
                    );
                    m.set(format!("pick_{i}_on"), on);
                }
                None => {
                    m.set(format!("pick_{i}"), "");
                    m.set(format!("pick_{i}_on"), false);
                }
            }
        }
        let last = (window + PICK_ROWS).min(cands.len());
        m.set(
            "pick_page",
            format!(
                "{}\u{2013}{} {} {}",
                (window + 1).min(last.max(1)),
                last,
                strings::resolve("$ap_of"),
                cands.len()
            ),
        );
        m.set("pick_prev_enabled", window > 0);
        m.set("pick_next_enabled", window + PICK_ROWS < cands.len());
    }

    /// FIT bindings (prop / garment): the paged socket picker + the offset / rotation / scale
    /// sliders. The Skin path never shows these — a non-character authors its single mount here in
    /// place of the six character attach points, so ONE Attach stage serves both, split by class.
    fn fit_model(&self, m: &mut ValueMap) {
        let fit = self.source.as_ref().map(|s| s.fit).unwrap_or_default();
        let window = self.source.as_ref().map(|s| s.fit_window).unwrap_or(0);
        for i in 0..SOCKET_ROWS {
            let idx = window + i;
            match SOCKETS.get(idx) {
                Some((_, label)) => {
                    let on = idx == fit.socket;
                    m.set(
                        format!("sock_{i}"),
                        format!(
                            "{}  {}",
                            if on { "\u{25c6}" } else { "\u{25c7}" },
                            strings::resolve(label)
                        ),
                    );
                    m.set(format!("sock_{i}_on"), on);
                }
                None => {
                    m.set(format!("sock_{i}"), "");
                    m.set(format!("sock_{i}_on"), false);
                }
            }
        }
        let last = (window + SOCKET_ROWS).min(SOCKETS.len());
        m.set(
            "sock_page",
            format!("{}\u{2013}{} {} {}", window + 1, last, strings::resolve("$ap_of"), SOCKETS.len()),
        );
        m.set("sock_prev_enabled", window > 0);
        m.set("sock_next_enabled", window + SOCKET_ROWS < SOCKETS.len());
        // "Mount:" reuses the step's own token — one copy source for the noun.
        m.set(
            "fit_socket",
            format!(
                "{}: {}",
                strings::resolve("$wf_step_mount"),
                strings::resolve(SOCKETS.get(fit.socket).map(|(_, l)| *l).unwrap_or("\u{2014}"))
            ),
        );
        let vals = [fit.offset[0], fit.offset[1], fit.offset[2], fit.rot[0], fit.rot[1], fit.rot[2]];
        for (i, (key, _)) in FIT_AXES.into_iter().enumerate() {
            m.set(key, vals[i] as f64);
        }
        // Per-axis scale reshapes; `fit_scale` is the scale-all that resizes without reshaping —
        // the paperdoll fit gadget's pair of controls.
        for (i, (key, _)) in FIT_SCALE_AXES.into_iter().enumerate() {
            m.set(key, fit.scale[i] as f64);
        }
        m.set("fit_scale", fit.uniform as f64);
    }

    /// REVIEW bindings: the summary rows and the engine-requirement checks, all real.
    fn review_model(&self, m: &mut ValueMap) {
        let reqs = self.requirements();
        for i in 0..REQUIREMENT_ROWS {
            match reqs.get(i) {
                Some((ok, text)) => {
                    m.set(format!("req_{i}"), format!("{}  {text}", if *ok { "\u{2713}" } else { "\u{2717}" }));
                    m.set(
                        format!("req_{i}_color"),
                        if *ok { "assetpipeline.map.ok" } else { "assetpipeline.map.fail" },
                    );
                }
                None => {
                    m.set(format!("req_{i}"), "");
                    m.set(format!("req_{i}_color"), "assetpipeline.map.ok");
                }
            }
        }
        let committed = self.source.as_ref().and_then(|s| s.committed.as_ref());
        m.set(
            "commit_label",
            strings::resolve(if committed.is_some() { "$ap_exported" } else { "$ap_export_to_pack_editor" })
                .into_owned(),
        );
        m.set("commit_enabled", reqs.iter().all(|(ok, _)| *ok));
        // Once a piece is baked, offer the loop straight back to the folder's picker — a weapon set
        // or an outfit is imported one piece at a time, and walking Back five stages to reach the
        // list again is not a workflow.
        let more = self.source.as_ref().map(|s| s.candidates.len() > 1).unwrap_or(false);
        m.set("has_committed", committed.is_some() && more);
    }

    fn inspector_title(&self) -> String {
        let token = match self.wf.step() {
            // Conform dispatches by role, so the inspector title tracks the rail: a prop reads
            // "Mount", an animation "Clips" — never the character "Rig" label over a page that is neither.
            "conform" => self.conform_role().title(),
            "attach" => "$ap_attach_points",
            "review" => "$wf_step_review",
            _ => "$wf_step_task", // "task"
        };
        strings::resolve(token).into_owned()
    }

    /// What the inspector shows for the current step. Only real, measured facts — a step
    /// whose processing is not wired says exactly that, so the panel can never read as
    /// done when it is not.
    fn inspector_lines(&self) -> Vec<String> {
        // Every static line is a `$token`; live values compose around the resolved text.
        let r = |t: &str| strings::resolve(t).into_owned();
        let Some(src) = self.source.as_ref() else {
            return vec![r("$ap_no_source_folder_open"), r("$ap_load_an_asset_folder_to_begin")];
        };
        if let Some(err) = src.error.as_ref() {
            return vec![r("$ap_blocked"), err.clone()];
        }
        let mut out = Vec::new();
        match self.wf.step() {
            // Conform / Attach / Review carry their own controls in the inspector, so these lines
            // are the EVIDENCE behind them — what the conform reports moved, what a socket resolved
            // against, and what the finished bake will carry.
            // A non-character was routed here: the Rig step is the CHARACTER path, so it is SKIPPED,
            // and the inspector states what Export will do instead — never an invented error.
            "conform" if matches!(src.class(), Some(AssetClass::Prop | AssetClass::Animation)) => {
                let cls = src.class();
                let is_clothing = cls == Some(AssetClass::Prop) && src.prop == PropKind::Clothing;
                out.push(format!("{} {}.", r("$ap_class"), class_label(cls)));
                out.push(String::new());
                out.push(r("$ap_rigging_maps_a_biped_skeleton_onto_the"));
                out.push(r("$ap_66_bone_reference_this_asset_has_none"));
                out.push(r("$ap_so_it_is_skipped"));
                out.push(String::new());
                match cls {
                    Some(AssetClass::Animation) => {
                        out.push(r("$ap_animation_clip_retarget_is_not_wired_yet"));
                    }
                    _ if is_clothing => {
                        out.push(r("$ap_export_skins_this_garment_onto_the_base"));
                        out.push(r("$ap_nearest_vertex_weight_transfer_fit_baked"));
                    }
                    _ => {
                        out.push(r("$ap_export_bakes_the_static_prop_mesh_its_so"));
                        out.push(r("$ap_and_fit_are_authored_in_the_paperdoll"));
                    }
                }
            }
            "conform" => match src.rig.as_ref() {
                None => out.push(r("$ap_conform_runs_when_this_stage_is_reached")),
                Some(rig) => {
                    // `{:<10}` / `{:<15}` reproduce the original aligned columns around
                    // the resolved words (en-us widths; other locales just realign).
                    out.push(format!("{:<10}{}", r("$ap_renamed"), rig.rename.renamed));
                    out.push(format!("{:<10}{}", r("$ap_dropped"), rig.rename.dropped));
                    out.push(format!("{:<10}{}", r("$ap_inferred"), rig.out.infer.added.len()));
                    out.push(format!("{:<15}{}", r("$ap_limbs_aligned"), rig.out.reorient.limbs_aligned));
                    let edited = rig.offsets.iter().filter(|o| !o.is_zero()).count();
                    out.push(format!("{:<15}{edited}", r("$ap_bones_edited")));
                    if !rig.rename.unmapped.is_empty() {
                        out.push(format!("{} {}", r("$ap_unmapped"), rig.rename.unmapped.join(", ")));
                    }
                }
            },
            "attach" => {
                let resolved = (0..src.attach.len()).filter(|i| self.attach_resolved(*i)).count();
                out.push(format!(
                    "{resolved} {} {} {}",
                    r("$ap_of"),
                    src.attach.len(),
                    r("$ap_points_resolve_to_a_bone")
                ));
                if src.rig.is_none() {
                    out.push(r("$ap_parents_resolve_after_conform_renames"));
                }
                if let Some(p) = src.attach.get(src.attach_sel) {
                    out.push(format!("{:<10}{}", r("$ap_socket_word"), p.id));
                    match self.attach_world(src.attach_sel) {
                        Some(w) => out.push(format!(
                            "{:<10}{:.1}, {:.1}, {:.1}",
                            r("$ap_world"),
                            w.x,
                            w.y,
                            w.z
                        )),
                        None => out.push(format!("{:<10}{}", r("$ap_world"), r("$ap_unresolved"))),
                    }
                }
                // The format gap is stated, not hidden: the points are authored against real
                // bones but `flicker.rig` has no list to persist them into yet.
                out.push(String::new());
                out.push(r("$ap_authored_here_flicker_rig_carries_a_sing"));
                out.push(r("$ap_attach_mount_so_the_set_is_not_persisted"));
            }
            "review" => {
                out.push(format!("{:<11}{}", r("$ap_asset"), src.asset_name()));
                out.push(format!("{:<11}{}", r("$ap_class_word"), class_label(src.class())));
                match src.parsed.as_ref() {
                    Some(p) => {
                        out.push(format!("{:<11}{} {}", r("$ap_skeleton"), p.bones(), r("$ap_bones")));
                        // The mesh-budget readout lives here now that Analyze is gone: game-ready is
                        // the input contract (decimation was dropped), so an over-budget source is
                        // REPORTED, never silently reduced.
                        out.push(format!("{:<11}{}", r("$ap_triangles"), p.tris));
                        out.push(if p.tris > TRI_BUDGET {
                            format!(
                                "{} {} {} ({} {TRI_BUDGET})",
                                r("$ap_over_budget"),
                                p.tris,
                                r("$ap_tris"),
                                r("$ap_target")
                            )
                        } else {
                            format!("{} {TRI_BUDGET}{}", r("$ap_within_the"), r("$ap_tri_budget"))
                        });
                    }
                    None => out.push(format!("{:<11}0 {}", r("$ap_skeleton"), r("$ap_bones"))),
                }
                out.push(format!("{:<11}{}", r("$ap_textures"), src.textures));
                match src.committed.as_ref() {
                    Some(p) => out.push(format!("{:<11}{}", r("$ap_exported_word"), p.display())),
                    None => out.push(r("$ap_not_exported")),
                }
            }
            // "task" is the entry page and carries no source yet, so the None early-return
            // above already handled it; this arm only absorbs it (and any future step id).
            _ => {}
        }
        out
    }

    /// The inspector's step-dependent badge — a one-word state, from the same real facts the
    /// body reports.
    fn inspector_badge(&self) -> String {
        let r = |t: &str| strings::resolve(t).into_owned();
        let Some(src) = self.source.as_ref() else { return String::new() };
        if src.error.is_some() {
            return r("$ap_badge_blocked");
        }
        match self.wf.step() {
            "conform" if matches!(src.class(), Some(AssetClass::Prop | AssetClass::Animation)) => {
                match src.class() {
                    Some(AssetClass::Animation) => r("$ap_badge_not_wired"),
                    _ if src.prop == PropKind::Clothing => r("$ap_badge_garment"),
                    _ => r("$ap_badge_static"),
                }
            }
            "conform" => match src.rig.as_ref() {
                None => r("$ap_badge_pending"),
                Some(rig) => {
                    let (ok, _, _) = rig.counts();
                    format!("{ok} / {}", rig.map.len())
                }
            },
            "attach" => format!("{} {}", src.attach.len(), r("$ap_badge_points")),
            "review" => {
                if self.requirements().iter().all(|(ok, _)| *ok) {
                    r("$ap_badge_passed")
                } else {
                    r("$ap_badge_blocked")
                }
            }
            _ => String::new(), // "task"
        }
    }

    /// Return to the rig view to import the NEXT piece. THE loop a weapon set or an outfit needs:
    /// pick → rig → bake → pick the next. Keeps the open folder and its candidate list; drops
    /// everything derived from the piece just committed, so the next one starts clean rather than
    /// inheriting the last one's rig or fit. The inline piece picker (`on_pick`) is right there on
    /// the rig stage for choosing which mesh comes next.
    fn start_next_piece(&mut self) {
        let Some(src) = self.source.as_mut() else { return };
        src.parsed = None;
        src.report = None;
        // Keep the DECLARED workflow: the next piece in a set is the same class the user chose on
        // the Workflow page, not a re-detection.
        src.class = self.pending_class;
        src.rig = None;
        src.committed = None;
        src.error = None;
        src.fit = PropFit::default();
        // Restart the SAME definition and land back on its rig view — the open folder
        // still satisfies the Task gate, so the fresh instance advances immediately.
        self.wf = Workflow::from_def(&self.wf_def);
        self.wf_advance();
    }

    /// The Conform stage's role for the loaded asset — the ONE place the class→role mapping is
    /// read, so the inspector, the footer copy and the HUD gating cannot drift apart.
    fn conform_role(&self) -> ConformRole {
        ConformRole::of(self.source.as_ref().and_then(|s| s.class()))
    }

    /// Start the workflow DEFINITION the declared class dispatches to — the launcher-field
    /// pattern: a Character (and the unclassified default) walks [`WF_CHARACTER`]'s four
    /// stops; a Prop / Accessory / Animation walks [`WF_PROP`], which simply HAS no
    /// character-only Attach step (the old `step_applies` skip, now a definition in data).
    fn dispatch_workflow(&mut self, class: Option<AssetClass>) {
        let name = match class {
            Some(AssetClass::Prop | AssetClass::Animation) => WF_PROP,
            Some(AssetClass::Skin) | None => WF_CHARACTER,
        };
        match self.workflows.get(name) {
            Some(def) => {
                self.wf_def = def.clone();
                self.wf = Workflow::from_def(&self.wf_def);
            }
            // `new()` proved the file loads, so this is a definition NAME drifting from the
            // content — warn and keep the running wizard rather than losing the user's place.
            None => tracing::warn!("workflow: no `{name}` in ui_workflows.json — keeping the current one"),
        }
    }

    /// This frame's step DOCUMENT — the workflow runtime's gating input, the `needs` /
    /// `yields` vocabulary of `ui_workflows.json`. A key is PRESENT exactly when the scene
    /// state it names exists; values are incidental (the runtime only probes presence).
    fn wf_doc(&self) -> ValueMap {
        let mut d = ValueMap::new();
        let Some(src) = self.source.as_ref() else { return d };
        d.set("source", true);
        if src.class().is_some() {
            d.set("class", true);
        }
        if src.rig.is_some() {
            d.set("rig", true);
        }
        if !src.attach.is_empty() {
            d.set("attach", true);
        }
        // The mount binding applies to (and is always authorable for) a Prop — a socket is
        // always selected and its placement defaults are valid. A Clip-role animation lacks
        // the key, which is what stops the wizard at its honestly-unwired page.
        if src.class() == Some(AssetClass::Prop) {
            d.set("fit", true);
        }
        if src.committed.is_some() {
            d.set("committed", true);
        }
        d
    }

    /// Step the running workflow forward once through its own gate — the SCENE-driven
    /// advance (`open` / `start_next_piece` landing past Task); the user-driven one is the
    /// footer button firing the same `wf_next` result through [`Self::apply_workflow_results`].
    fn wf_advance(&mut self) {
        let doc = self.wf_doc();
        self.wf.handle(&ValueMap::new().with("wf_next", true), &doc);
    }

    /// Consume this frame's workflow results. PROGRESSION is the runtime's
    /// (`Workflow::handle`: the needs gate, Back, the dirty discard guard); the two arms
    /// that were always SCENE policy stay here:
    /// - "Restart" — the last page's forward button loops back to the Workflow page for
    ///   the next asset (the runtime treats a last-step `wf_next` as the bench's own).
    /// - Back on the FIRST page clears the open folder — there is no earlier step, and
    ///   without it a wrongly-chosen folder could not be abandoned without leaving the scene.
    fn apply_workflow_results(&mut self, results: &ValueMap) {
        if results.is_on("wf_next") && self.wf.step() == "review" {
            // Drop the finished asset and return to the Task page — the true entry, where
            // the next workflow is re-declared. GPU/theme/input infra is kept; only the
            // per-asset wizard state (and the declared class → the default definition)
            // resets.
            self.pending_class = None;
            self.pending_prop = None;
            self.source = None;
            self.grid = None;
            self.quad_rect = None;
            self.dispatch_workflow(None);
            return;
        }
        if results.is_on("wf_back") && self.wf.step() == "task" {
            self.source = None;
        }
        let doc = self.wf_doc();
        self.wf.handle(results, &doc);
    }

    /// Apply the per-stage HUD results: the exclusive selections, the list navigation, and the
    /// offset sliders. Kept out of `update` so the wizard's own flow stays readable.
    ///
    /// The skeleton is re-derived ONLY when an authored value actually changed — the sliders
    /// report their value every frame, so comparing before rebuilding is what keeps this off the
    /// per-frame path. Returns whether an AUTHORED value changed this frame — the caller arms
    /// the workflow's Back discard guard from it (selection and paging are navigation, not
    /// edits, so they never set it; picking a different piece RESETS rather than edits).
    fn apply_stage_results(&mut self, results: &ValueMap) -> bool {
        let Some(src) = self.source.as_mut() else { return false };
        let mut edited = false;

        // The inline piece picker (rig stage). Choosing a DIFFERENT piece invalidates everything
        // derived from the previous one, so the wizard can never carry a stale parse/conform forward.
        for i in 0..PICK_ROWS {
            if results.is_on(&format!("pick_sel_{i}")) {
                let idx = src.pick_window + i;
                if idx < src.candidates.len() && idx != src.candidate_sel {
                    src.candidate_sel = idx;
                    src.fbx = src.candidates[idx].clone();
                    src.parsed = None;
                    src.report = None;
                    // Preserve the declared workflow across a pick — only the derived state is stale.
                    src.class = self.pending_class;
                    src.rig = None;
                    src.committed = None;
                    src.error = None;
                }
            }
        }
        if results.is_on("pick_prev") {
            src.pick_window = src.pick_window.saturating_sub(PICK_ROWS);
        }
        if results.is_on("pick_next") && src.pick_window + PICK_ROWS < src.candidates.len() {
            src.pick_window += PICK_ROWS;
        }

        // Attach — point selection, then the three offset sliders for the selected point.
        for i in 0..src.attach.len() {
            if results.is_on(&format!("att_sel_{i}")) {
                src.attach_sel = i;
            }
        }
        let sel = src.attach_sel;
        if let Some(p) = src.attach.get_mut(sel) {
            for (i, (key, _)) in ATTACH_AXES.into_iter().enumerate() {
                if let Some(v) = results.number(key) {
                    let v = v as f32;
                    if p.offset[i] != v {
                        p.offset[i] = v;
                        edited = true;
                    }
                }
            }
        }

        // Fit (prop / garment) — socket selection, socket paging, and the offset/rotation/scale
        // sliders. BEFORE the Conform early-return below, since a non-character carries no rig and
        // this is the only stage where its placement is authored.
        for i in 0..SOCKET_ROWS {
            if results.is_on(&format!("sock_sel_{i}")) {
                let idx = src.fit_window + i;
                // Re-mounting to a DIFFERENT socket is a fit edit (the bake carries it);
                // re-clicking the current one is not.
                if idx < SOCKETS.len() && idx != src.fit.socket {
                    src.fit.socket = idx;
                    edited = true;
                }
            }
        }
        if results.is_on("sock_prev") {
            src.fit_window = src.fit_window.saturating_sub(SOCKET_ROWS);
        }
        if results.is_on("sock_next") && src.fit_window + SOCKET_ROWS < SOCKETS.len() {
            src.fit_window += SOCKET_ROWS;
        }
        // Offset/rotation ride FIT_AXES order (three offsets, then three rotations); every
        // write is change-checked — the sliders report every frame, and a same-value report
        // must neither arm the discard guard nor count as an edit.
        for (i, (key, _)) in FIT_AXES.into_iter().enumerate() {
            if let Some(v) = results.number(key) {
                let v = v as f32;
                let slot = if i < 3 { &mut src.fit.offset[i] } else { &mut src.fit.rot[i - 3] };
                if *slot != v {
                    *slot = v;
                    edited = true;
                }
            }
        }
        for (i, (key, _)) in FIT_SCALE_AXES.into_iter().enumerate() {
            if let Some(v) = results.number(key) {
                let v = (v as f32).max(0.01);
                if src.fit.scale[i] != v {
                    src.fit.scale[i] = v;
                    edited = true;
                }
            }
        }
        if let Some(v) = results.number("fit_scale") {
            let v = (v as f32).max(0.01);
            if src.fit.uniform != v {
                src.fit.uniform = v;
                edited = true;
            }
        }

        // Conform — bone selection, paging, the four offset sliders, and Reset.
        let Some(rig) = src.rig.as_mut() else { return edited };
        let total = rig.map.len();
        for i in 0..BONE_ROWS {
            if results.is_on(&format!("bone_sel_{i}")) && rig.window + i < total {
                rig.sel = rig.window + i;
                rig.focused = true; // picking a bone from the list focuses it (draws the gizmo).
            }
        }
        // Paging moves the SELECTION by a page and lets the window follow it. One cursor, not
        // two: a window that scrolled independently would fight `scroll_to_selection` and snap
        // straight back, and the offset sliders would address a bone that is no longer on screen.
        if results.is_on("bone_prev") {
            rig.sel = rig.sel.saturating_sub(BONE_ROWS);
        }
        if results.is_on("bone_next") {
            rig.sel = (rig.sel + BONE_ROWS).min(total.saturating_sub(1));
        }
        Self::scroll_to_selection(rig);

        let before = rig.offsets.get(rig.sel).copied().unwrap_or_default();
        let mut after = before;
        for (i, (key, _)) in OFFSET_AXES.into_iter().enumerate() {
            if let Some(v) = results.number(key) {
                if i < 3 {
                    after.t[i] = v as f32;
                } else {
                    after.roll = v as f32;
                }
            }
        }
        if results.is_on("bone_reset") {
            after = BoneOffset::default();
        }
        if after != before {
            if let Some(slot) = rig.offsets.get_mut(rig.sel) {
                *slot = after;
            }
            // The authored pose changed — re-derive the frames once, here, not per frame.
            let offsets = rig.offsets.clone();
            if let Some(parsed) = src.parsed.as_mut() {
                parsed.rebuild(&offsets);
            }
            // Bump the live-skin cache key so the Conform viewport re-skins with the slider's edit,
            // exactly as a gizmo drag does — the two write the same offset.
            self.pose_gen = self.pose_gen.wrapping_add(1);
            edited = true;
        }
        edited
    }

    /// Attach-point markers for the viewport, split into (unselected, selected) so each draws in
    /// its own colour. Empty until the Attach stage is reached — a marker on the Analyze view
    /// would claim a placement that has not been authored.
    ///
    /// A marker is a three-axis cross rather than a ring: the content is Z-up and the views are
    /// axis-aligned, so a cross is the one shape that reads in all four of them.
    fn attach_markers(&self, radius: f32) -> (Segments, Segments) {
        let Some(src) = self.source.as_ref() else { return (Vec::new(), Vec::new()) };
        let step = self.wf.step();
        if step != "attach" && step != "review" {
            return (Vec::new(), Vec::new());
        }
        let r = radius * 0.04;
        let (mut plain, mut sel) = (Vec::new(), Vec::new());
        for i in 0..src.attach.len() {
            let Some(c) = self.attach_world(i) else { continue };
            let scale = if i == src.attach_sel { r * 1.6 } else { r };
            let cross = [
                (c - Vec3::X * scale, c + Vec3::X * scale),
                (c - Vec3::Y * scale, c + Vec3::Y * scale),
                (c - Vec3::Z * scale, c + Vec3::Z * scale),
            ];
            if i == src.attach_sel {
                sel.extend_from_slice(&cross);
            } else {
                plain.extend_from_slice(&cross);
            }
        }
        (plain, sel)
    }

    /// World position of the currently-authored attach point `i` — see [`Source::attach_world`].
    fn attach_world(&self, i: usize) -> Option<Vec3> {
        self.source.as_ref()?.attach_world(i)
    }

    /// Move the bone-map window so the selected row is visible — selecting from the viewport or
    /// stepping pages keeps the two in agreement.
    fn scroll_to_selection(rig: &mut Rig) {
        if rig.sel < rig.window {
            rig.window = rig.sel;
        } else if rig.sel >= rig.window + BONE_ROWS {
            rig.window = rig.sel + 1 - BONE_ROWS;
        }
    }
}

/// How many inspector lines the HUD tree declares. Kept in one place so the Lua and the
/// model agree.
const INSPECTOR_LINES: usize = 8;

/// Target triangle budget for a character. Sources are expected to arrive game-ready
/// (Aaron 2026-07-22 — the in-app decimate stage was dropped), so this is a REPORTING
/// threshold, not something the pipeline enforces or corrects.
const TRI_BUDGET: usize = 4_000;

/// The canonical rig's bone count — the BAKED figure, which is what `flicker.rig` carries and
/// what every other part of the engine quotes. Canon value; the
/// `reference_rig_still_has_the_canonical_bone_count` test asserts it against the reference file
/// itself, so this cannot drift away from the content it describes.
const REFERENCE_BONES: usize = 66;

/// What a CONFORMED model carries, one short of the canonical count: `root` is synthesized at
/// bone 0 by `bake_rig`, not by conform, so a 65-bone conform result is complete. Deriving it
/// here keeps the two figures from being independently maintained.
const CONFORMED_BONES: usize = REFERENCE_BONES - 1;

/// Visible rows in the bone map. The list is 66 rows in a 150px box, so it pages rather than
/// truncating — every bone stays reachable.
const BONE_ROWS: usize = 6;

/// How many requirement rows the Review tree declares.
const REQUIREMENT_ROWS: usize = 4;

/// The Conform stage's four offset sliders: their Model key and their label. Slider range is
/// symmetric about zero, so the conform result sits at the centre of every track.
const OFFSET_AXES: [(&str, &str); 4] =
    [("off_x", "Translate X"), ("off_y", "Translate Y"), ("off_z", "Translate Z"), ("off_roll", "Roll")];

/// The Attach stage's three offset sliders.
const ATTACH_AXES: [(&str, &str); 3] = [("att_x", "Off X"), ("att_y", "Off Y"), ("att_z", "Off Z")];

/// Visible rows in the prop/garment socket picker (must equal the `clayworks_bank` proto's
/// fixed row count, which the drift gate asserts).
const SOCKET_ROWS: usize = 6;

/// Visible rows in the Load stage's riggable-mesh picker (must equal `PICK_ROWS` in the lua).
const PICK_ROWS: usize = 6;

/// The six offset/rotation sliders of the prop/garment fit stage (scale is a separate non-symmetric
/// slider). Offsets are cm, rotations are degrees; both tracks are symmetric about zero so the
/// unfitted asset sits at the centre.
const FIT_AXES: [(&str, &str); 6] = [
    ("fit_ox", "Offset X"),
    ("fit_oy", "Offset Y"),
    ("fit_oz", "Offset Z"),
    ("fit_rx", "Rotate X"),
    ("fit_ry", "Rotate Y"),
    ("fit_rz", "Rotate Z"),
];

/// The fit stage's PER-AXIS scale sliders, in `PropFit::scale` index order. The scale-ALL slider is
/// separate and keeps the `fit_scale` binding (so it reshapes nothing on its own).
const FIT_SCALE_AXES: [(&str, &str); 3] =
    [("fit_sx", "Scale X"), ("fit_sy", "Scale Y"), ("fit_sz", "Scale Z")];

/// A batch of world-space line segments, in the shape `Renderer::draw_lines_overlay` takes.
type Segments = Vec<(Vec3, Vec3)>;

/// The exclusive-choice glyph pair. The walker's component set has no radio, and a radio IS a
/// mutually-exclusive button whose state Rust already owns — so the selection rides the same
/// pre-formatted-string channel as the step rail rather than growing the component registry.
fn radio(on: bool) -> &'static str {
    if on {
        "\u{25c9}"
    } else {
        "\u{25cb}"
    }
}

/// The user-facing name for an asset class. `AssetClass::id()` ("skin"/"prop"/"animation") is a
/// stable serialization token and must never reach the UI; this is the DISPLAY string, kept separate
/// so the id cannot leak into a panel and so localization has a single place to hook. Skin reads as
/// "Character" — the word the user chose on the workflow card, not the internal skin term.
fn class_label(class: Option<AssetClass>) -> Cow<'static, str> {
    strings::resolve(match class {
        Some(AssetClass::Skin) => "$ap_character",
        Some(AssetClass::Prop) => "$ap_prop",
        Some(AssetClass::Animation) => "$ap_animation",
        None => "$ap_unclassified",
    })
}

/// CPU linear-blend skin of the source mesh into deformed [`MeshVertex`]es (source space), through
/// the posed `globals` and each bone's `inverse_bind`. Mirrors `flicker-skeletal::skin` but over the
/// raw [`RawModel`] the pipeline holds — no format conversion. The palette is `globals[b] *
/// inverse_bind[b]`; at the conform rest pose that is the identity (so the bind mesh is reproduced),
/// and an authored offset moves the bone's `globals` entry, deforming the vertices it weights.
fn skin_source_verts(model: &RawModel, globals: &[Mat4]) -> Vec<MeshVertex> {
    let palette: Vec<Mat4> = model
        .bones
        .iter()
        .enumerate()
        .map(|(b, bone)| {
            globals.get(b).copied().unwrap_or(Mat4::IDENTITY) * Mat4::from_cols_array(&bone.inverse_bind)
        })
        .collect();
    model
        .vertices
        .iter()
        .map(|v| {
            let p = Vec3::from_array(v.p).extend(1.0);
            let n = Vec3::from_array(v.n);
            let mut pos = Vec3::ZERO;
            let mut nrm = Vec3::ZERO;
            let mut any = false;
            for k in 0..4 {
                let w = v.weights[k];
                if w == 0.0 {
                    continue;
                }
                any = true;
                let m = palette.get(v.joints[k] as usize).copied().unwrap_or(Mat4::IDENTITY);
                pos += w * (m * p).truncate();
                nrm += w * (glam::Mat3::from_mat4(m) * n);
            }
            let position = if any { pos.to_array() } else { v.p };
            let normal = if nrm.length_squared() > 1e-12 { nrm.normalize().to_array() } else { v.n };
            MeshVertex { position, normal, material: 0 }
        })
        .collect()
}

/// Group per-segment-coloured gizmo lines by colour, so each colour issues as one
/// `draw_lines_overlay` batch (the renderer takes ONE colour per call). A handful of colours, so a
/// linear scan beats a map.
fn group_by_color(segs: Vec<(Vec3, Vec3, [f32; 4])>) -> Vec<([f32; 4], Segments)> {
    let mut groups: Vec<([f32; 4], Segments)> = Vec::new();
    for (a, b, c) in segs {
        if let Some(g) = groups.iter_mut().find(|(gc, _)| *gc == c) {
            g.1.push((a, b));
        } else {
            groups.push((c, vec![(a, b)]));
        }
    }
    groups
}

/// Rest-pose world frames + parent topology from a parsed model, with the editor's authored
/// `offsets` folded in. Bones are stored as local TRS relative to their parent, so a single
/// forward pass composes them; parents always precede children in an FBX skeleton.
///
/// An offset is parent-relative translation plus a roll about the bone's own X axis — the same
/// space the source bone is stored in, so an offset of zero reproduces the conform exactly.
fn rest_globals(model: &RawModel, offsets: &[BoneOffset]) -> (Vec<Mat4>, Vec<i32>) {
    let mut globals: Vec<Mat4> = Vec::with_capacity(model.bones.len());
    let mut parents: Vec<i32> = Vec::with_capacity(model.bones.len());
    for (i, b) in model.bones.iter().enumerate() {
        let o = offsets.get(i).copied().unwrap_or_default();
        let local = Mat4::from_scale_rotation_translation(
            Vec3::from_array(b.scale),
            glam::Quat::from_array(b.rotation) * glam::Quat::from_rotation_x(o.roll.to_radians()),
            Vec3::from_array(b.translation) + Vec3::from_array(o.t),
        );
        let world = match usize::try_from(b.parent) {
            Ok(p) if p < globals.len() => globals[p] * local,
            _ => local,
        };
        globals.push(world);
        parents.push(b.parent);
    }
    (globals, parents)
}

/// Fold the authored offsets into a model's bones, so a bake carries what the viewport showed.
/// The same arithmetic as `rest_globals` applies, one level down: local TRS, parent-relative.
fn apply_offsets(model: &mut RawModel, offsets: &[BoneOffset]) {
    for (b, o) in model.bones.iter_mut().zip(offsets) {
        if o.is_zero() {
            continue;
        }
        for k in 0..3 {
            b.translation[k] += o.t[k];
        }
        let q = glam::Quat::from_array(b.rotation)
            * glam::Quat::from_rotation_x(o.roll.to_radians());
        b.rotation = q.to_array();
    }
}

/// A WORLD-space translation delta expressed in a bone's PARENT-local frame (root = world), using the
/// pre-edit `globals` for the parent's rotation — the space `RawBone::translation` / `BoneOffset::t`
/// live in.
fn parent_local_delta(globals: &[Mat4], model: &RawModel, sel: usize, world_delta: Vec3) -> Vec3 {
    let parent_inv = model
        .bones
        .get(sel)
        .and_then(|b| usize::try_from(b.parent).ok())
        .and_then(|pi| globals.get(pi))
        .map(|g| glam::Mat3::from_mat4(*g).inverse())
        .unwrap_or(glam::Mat3::IDENTITY);
    parent_inv * world_delta
}

/// The symmetric bone NAME for a left/right bone (`thigh_l`↔`thigh_r`), or `None` for a centre bone.
fn mirror_name(name: &str) -> Option<String> {
    const PAIRS: [(&str, &str); 6] =
        [("_l", "_r"), ("_r", "_l"), ("_L", "_R"), ("_R", "_L"), (".L", ".R"), (".R", ".L")];
    for (a, b) in PAIRS {
        if let Some(stem) = name.strip_suffix(a) {
            return Some(format!("{stem}{b}"));
        }
    }
    None
}

/// Per-bone provenance, read straight out of the conform reports — the bone map's colour key has
/// exactly one source of truth.
///
/// `InferReport.added` names the bones the reference contributed (auto); the hip / shoulder /
/// ankle derives moved joints whose placement is worth a human's eye (review); everything else
/// came from the source and was renamed (ok).
fn bone_map_states(model: &RawModel, out: &ConformOutput) -> Vec<MapState> {
    // The derive passes report per-side placements, not bone names, so a side that was actually
    // placed marks its own joints.
    let mut review: Vec<&str> = Vec::new();
    let mut mark = |placed: bool, names: &[&'static str]| {
        if placed {
            review.extend_from_slice(names);
        }
    };
    mark(out.hip.left.is_some(), &["thigh_l"]);
    mark(out.hip.right.is_some(), &["thigh_r"]);
    mark(out.shoulder.left.is_some(), &["clavicle_l", "upperarm_l"]);
    mark(out.shoulder.right.is_some(), &["clavicle_r", "upperarm_r"]);
    mark(out.ankle.left.is_some(), &["foot_l", "ball_l"]);
    mark(out.ankle.right.is_some(), &["foot_r", "ball_r"]);

    model
        .bones
        .iter()
        .map(|b| {
            if out.infer.added.iter().any(|a| a == &b.name) {
                MapState::Auto
            } else if review.contains(&b.name.as_str()) {
                MapState::Review
            } else {
                MapState::Ok
            }
        })
        .collect()
}

/// Where a committed rig lands — **STAGING**, not the shipped package.
///
/// A commit here is the pipeline's OUTPUT, not a publish: the Content Manager bench reviews what
/// lands in staging and promotes it into `package/` (recording it in the package manifest). So a
/// fresh commit is deliberately NOT visible to the running game until it is promoted.
///
/// The root comes from the executable's `content.json` via [`flicker_content::roots`] rather than a
/// climb out of this crate's source dir, so the tree can move without touching this bench.
fn characters_dir() -> PathBuf {
    flicker_content::roots().staging().join("characters")
}

/// The asset's bounding CENTRE and half-extent — the framing the viewport needs.
///
/// Measured from the MESH when there is one (a prop carries no skeleton at all) and from the bone
/// frames otherwise. Everything is then drawn offset by `-centre`, because the quad cameras all
/// target the ORIGIN and in Z-up ground reckoning the origin is the asset's FEET (a character
/// stands 0..170 in +Z) — targeting it framed the feet with the body sticking out of shot.
fn model_bounds(model: &RawModel, globals: &[Mat4]) -> (Vec3, f32, f32) {
    let (mut lo, mut hi) = (Vec3::splat(f32::INFINITY), Vec3::splat(f32::NEG_INFINITY));
    let mut any = false;
    for v in &model.vertices {
        let p = Vec3::from(v.p);
        lo = lo.min(p);
        hi = hi.max(p);
        any = true;
    }
    if !any {
        for g in globals {
            let p = g.w_axis.truncate();
            lo = lo.min(p);
            hi = hi.max(p);
            any = true;
        }
    }
    if !any {
        return (Vec3::ZERO, 1.0, 0.0);
    }
    let centre = (lo + hi) * 0.5;
    // The floor is reported ALREADY RECENTRED (`lo.z - centre.z`, so it is negative), because
    // every caller draws through the same `-centre` offset — handing back the raw `lo.z` would
    // make each one re-derive the shift and eventually one of them would forget.
    (centre, ((hi - lo).max_element() * 0.5).max(1.0), lo.z - centre.z)
}

/// Build the Clayworks Bench editor as a boxed [`Scene`] for the `prism-alpha` launcher.
pub fn scene() -> Box<dyn Scene> {
    Box::new(AssetPipeline::new())
}

impl Scene for AssetPipeline {
    fn enter(&mut self, renderer: &mut Renderer) {
        // 1×1 white pixel — `render_hud` tints it to build the HUD's solid quads.
        self.hud_white = Some(renderer.load_texture(&[0xff, 0xff, 0xff, 0xff], 1, 1));
        // The shared 2×2 editor viewport (Perspective TL, Top TR, Side BL, Front BR) —
        // the same grid the paperdoll uses, owned by flicker-render.
        self.grid = Some(QuadGrid::editor(renderer));
        // Built once and handed to each PauseScene we push, so pausing never re-uploads.
        self.ui_theme = Some(Theme::build(renderer));
        // PRE-LOAD the fitting body (the clay Golem) WITH the scene — mesh and all — so turning
        // the reference body on for a prop or an outfit is instant instead of a first-fit hitch.
        self.base = BasePreview::load();
        // Split the borrows: the loaded body is READ while the texture cache is filled.
        let Self { base, textures, base_upload, .. } = self;
        if let Some(b) = base.as_ref() {
            if !b.verts.is_empty() {
                let up = upload_preview(renderer, textures, &b.maps, &b.verts, &b.uvs, &b.indices);
                tracing::info!(
                    verts = b.verts.len(),
                    textured = matches!(up, Uploaded::Textured { .. }),
                    "asset pipeline: fitting body uploaded"
                );
                *base_upload = Some(up);
            }
        }
    }

    fn update(&mut self, _dt: Duration, input: &InputState, renderer: &Renderer) -> Transition {
        // The HUD walks first: it lays out the framed holder, and the rect it reserves for the 2×2 is
        // what the viewport controls below pick against — a cursor outside the holder (over the editor
        // rail or a bar) lands on no view, so the chrome no longer has to "claim" the pointer. Its
        // `hud_hit` (the walker's canonical "pointer over UI" verdict) feeds the router's WalkerHandler
        // layer in the dispatch below; the raw Esc handling that used to sit at the top of this
        // function now moves onto the bus (the scene-root `Menu` consume, spec section 9).
        let screen = renderer.size();
        // The HUD is unconditional now — there is no "script failed to load" arm that
        // leaves the scene treeless, because the tree is built from data this crate
        // ships. `hud_hit` is therefore always the walker's real verdict.
        let hud_hit;
        {
            // Composition is DATA: the tree is re-emitted and re-expanded every frame
            // from `clayworks_bench`, so a template param tracks the model instead of
            // being frozen at construction.
            let tree = self.build_tree(screen);
            self.ui_intents = UiIntents::of(&tree);
            let model = self.hud_model();
            let snap = UiInput {
                mouse: input.mouse_position,
                clicked: input.mouse_left_pressed,
                down: input.mouse_left,
                screen,
                typed: String::new(),
                backspace: false,
                wheel: input.mouse_wheel_delta,
            };
            let frame = {
                // Disjoint field borrows: `ui_styles` / `ui_host` read, `ui_state` mutated.
                // `lib` routes a node's draw to its Lua component twin when the host
                // `handles` its kind (buttons today); `None` would be today's all-Rust path.
                let lib = self.ui_host.as_ref().map(|h| h as &dyn ComponentLibrary);
                run_ui_with(&tree, &model, &self.ui_styles, &snap, &mut self.ui_state, lib)
            };
            self.hud_commands = frame.commands;
            // The framed holder the HUD reserved for the 2×2 (the `editor_quad` stage node): the grid
            // tiles inside exactly this rect, so the four views land in the frame and the rail sits
            // beside them. Setting it on the grid keeps the composite and the pointer-picking in step.
            let viewport = frame
                .rtts
                .iter()
                .find(|s| s.id == "editor_quad")
                .map(|s| Rect { pos: Vec2::new(s.x, s.y), size: Vec2::new(s.w, s.h) });
            self.quad_rect = viewport;
            if let Some(g) = self.grid.as_mut() {
                g.set_viewport(viewport);
            }
            let results = frame.results;
            // The walker's canonical "pointer over UI" verdict — handed to the WalkerHandler
            // layer in the dispatch below so the event bus, not a hand-rolled gate, owns
            // HUD-consume (spec section 9: `hud_hit` is the router mechanism, not legacy).
            hud_hit = results.is_on("hud_hit");
            self.show_skeleton = results.is_on("show_skeleton");
            self.show_base = results.is_on("show_base");
            self.show_collision = results.is_on("show_collision");
            self.apply_gizmo_results(&results);
            // Joint symmetry — read only on Conform, where the checkbox lives (off the stage `is_on`
            // reads false and would clear it). Bake Skin re-weights the mesh to the repositioned rig.
            if self.wf.step() == "conform" {
                self.mirror_joints = results.is_on("mirror");
            }
            if results.is_on("bake_skin") {
                self.bake_skin_now();
            }

            // Import panel — the four workflow cards ("CHOOSE A THREAD"). Each DECLARES the class
            // (+ Prop sub-type) the user is importing, never auto-detected, then opens the OS folder
            // dialog IMMEDIATELY — the card IS the trigger, there is no separate Load button. On a
            // successful pick the existing `open()` runs parse+conform+land inline (Character → the
            // gizmo rig view); a CANCELLED pick leaves `source == None`, so we fall back to the panel.
            // Read here in `update`, NOT `apply_stage_results` (which early-returns with no source).
            let card = if results.is_on("import_character") {
                Some((Some(AssetClass::Skin), None))
            } else if results.is_on("import_accessory") {
                // Worn things — bound to a bearer. Garment bake (`write_garment`).
                Some((Some(AssetClass::Prop), Some(PropKind::Clothing)))
            } else if results.is_on("import_prop") {
                // Standalone objects of the world — the static bake (`write_prop`).
                Some((Some(AssetClass::Prop), Some(PropKind::Environment)))
            } else if results.is_on("import_animation") {
                Some((Some(AssetClass::Animation), None))
            } else {
                None
            };
            if let Some((class, prop)) = card {
                self.pending_class = class;
                self.pending_prop = prop;
                self.source = None;
                // `open()` dispatches the declared class's workflow definition and lands the
                // scanned folder on its rig view (a scan error surfaces there as the
                // inspector's "Blocked:" line). A CANCELLED dialog leaves `source == None`
                // and never reaches `open`, so the wizard simply stays parked on Task —
                // the only page the cards are clickable from.
                self.load_folder();
            }
            // The workflow results (`wf_next` / `wf_back` / the discard pair): progression
            // is the runtime's; the Restart and clear-folder arms are scene policy.
            self.apply_workflow_results(&results);
            if results.is_on("commit") {
                self.commit();
            }
            if results.is_on("next_piece") {
                self.start_next_piece();
            }
            // Step edits arm the Back discard guard; advancing clears it in the runtime.
            if self.apply_stage_results(&results) {
                self.wf.set_dirty(true);
            }
        }

        // ── Input seam (spec section 9 port): resolve this frame's discrete edges over the
        // active context, wrap each `Fired` as an `InputEvent`, then dispatch through the
        // chain (scene root -> HUD walker). `ev` is the REUSED scratch buffer (no per-frame
        // alloc — RT-7); `events` is a short-lived local because it borrows THIS frame's
        // snapshot. The 2×2 viewport orbit/pan/zoom below stays POLLED off the bus: the
        // `editor_quad` holder is a styled `stage` (so `hud_hit` is already true over the
        // viewport itself), and the controls gate on `QuadGrid::cell_at`, not on a routed
        // signal — the bespoke per-panel picking the group note says to keep. ──
        self.tick = self.tick.wrapping_add(1);
        self.ev.clear();
        self.resolver
            .resolve_frame(&self.bindings, &self.gamepad_config, input, self.tick, &mut self.ev);
        let ctx = self.bindings.active();
        let events: Vec<InputEvent> = self
            .ev
            .iter()
            .map(|f| InputEvent::from_fired(f, ctx, input))
            .collect();

        self.fired_sigs.clear(); // last frame's mirror rode the HUD walk above — done
        let mut root = RootHandler;
        let mut walker =
            WalkerHandler::hud(&mut self.ui_state, hud_hit).with_intents(&self.ui_intents);
        {
            let mut chain: [&mut dyn InputHandler; 2] = [&mut root, &mut walker];
            Router::dispatch(&events, &mut chain, &mut self.route);
        }
        // Reconcile router-owned intents THROUGH the walker (one focus id for mouse +
        // gamepad, spec section 4.2a/4.3). This trivial scene pushes no context and requests
        // no focus, so it is a no-op today, but keeping the seam standard means a future
        // handler that does just works.
        let focus_change = apply_context_requests(&mut self.bindings, &self.route.requests);
        walker.apply_focus(focus_change);
        // The screen's fired intents (S9), drained once: acted on below and queued
        // for the one-frame `sig_<name>` Model mirror.
        self.fired_sigs = walker.take_fired();
        self.route.requests.clear();

        // The screen DECLARED `on_menu = "pause_open"` (S9): the walker layer consumed
        // the Menu press and fired the name; the scene maps it onto the shell pause
        // overlay (Resume / Settings / Main Menu / Quit), the only supported way back
        // out of a scene. The root's hardcoded Menu arm is gone.
        if self.fired_sigs.iter().any(|n| n == "pause_open") {
            if let Some(theme) = self.ui_theme {
                return Transition::Push(Box::new(PauseScene::new(
                    theme,
                    self.bindings.active_map(),
                    &self.controls,
                    &self.gamepad_config,
                )));
            }
        }

        // A stage runs as soon as its step is reached and its input exists. Each is guarded
        // against re-running, so this is a no-op on every later frame.
        // Entering the rig view: parse then conform INLINE. The single-mesh Load already ran both
        // in `open`; the multi-mesh Load→Conform hop runs them here. Both are idempotent (analyze
        // no-ops once parsed, conform once rigged and early-returns for Prop/Animation), so this is
        // safe to reach every frame.
        if self.wf.step() == "conform" {
            self.analyze();
            self.conform();
        }

        // A left-click on an ORTHO panel's corner label flips that view to its opposite side
        // (LEFT↔RIGHT, TOP↔BOTTOM, FRONT↔BACK). The control now lives ON the panel, beside the label
        // it toggles — which updates immediately — instead of as an inspector checkbox. PERSP has no
        // fixed side, so it is never a flip target. `QuadGrid::flipped` stays the single source of
        // truth: the click writes it and the composited label reads it, so text and view can't drift.
        let flip_target = if input.mouse_left_pressed && self.ui_state.drag().is_none() {
            self.grid.as_ref().and_then(|g| {
                g.cell_at(input.mouse_position, screen).filter(|&i| {
                    g.views()[i].ortho.is_some() && g.label_hit(i, input.mouse_position, screen)
                })
            })
        } else {
            None
        };
        if let Some(i) = flip_target {
            if let Some(g) = self.grid.as_mut() {
                if let Some(f) = g.flipped.get_mut(i) {
                    *f = !*f;
                }
            }
        }

        // The in-scene transform gizmo picks/drags BEFORE the orbit, so a joint or handle grab wins
        // the gesture; `gizmo` is true while it is handling the pointer, and suppresses the orbit on
        // the same drag (a pick that hit nothing leaves the orbit free).
        let gizmo = if flip_target.is_none() {
            self.gizmo_interact(input, screen)
        } else {
            false
        };

        // Viewport controls act on the quad UNDER THE CURSOR only, each on its OWN camera — so
        // panning/zooming/orbiting one panel leaves the other three put.
        let delta = input.mouse_position - self.last_mouse;
        self.last_mouse = input.mouse_position;
        if self.ui_state.drag().is_none() {
            if let Some(i) = self.grid.as_ref().and_then(|g| g.cell_at(input.mouse_position, screen)) {
                // Orbit (left-drag) is a PERSPECTIVE notion — only the PERSP panel (view 0) rotates;
                // the ortho views are fixed axes. A flip-label click is not an orbit, and neither is
                // a gizmo pick/drag.
                if input.mouse_left && flip_target.is_none() && i == 0 && !gizmo {
                    self.orbits[0].yaw -= delta.x * 0.006;
                    self.orbits[0].pitch = (self.orbits[0].pitch + delta.y * 0.006).clamp(-1.4, 1.4);
                }
                // PAN (right-drag; a two-finger trackpad drag arrives as one) slides THIS view's
                // look-at in its own plane — TOP across XY, FRONT across XZ. Each quad is half the
                // HOLDER's height now, so that is what a pixel is measured against.
                if input.mouse_right {
                    let cam = self.view_camera_at(input.mouse_position, screen);
                    let quad_h = self.quad_rect.map(|r| r.size.y).unwrap_or(screen.y);
                    self.orbits[i].pan_by_view(delta, &cam, quad_h * 0.5);
                }
                // Wheel zooms THIS view only.
                self.orbits[i].zoom_by(input.mouse_wheel_delta);
            }
        }

        Transition::None
    }

    fn render(&mut self, renderer: &mut Renderer) {
        let base_layer = renderer.layer();

        // The four views FIRST. `QuadGrid::render` drives the shared `FrameGraph`, whose
        // offscreen passes RESET the per-frame draw queues (the centralized "render RTTs
        // before the main view" rule) — so anything queued before this is discarded, and
        // the HUD must come after. The views composite one layer above the backdrop.
        // The skeleton is drawn as an overlay so it reads through anything in front of it.
        // Prop/garment fit preview (the base rig + the imported mesh at socket·fit). Built FIRST —
        // its mesh upload needs `&mut Renderer` before grid.render borrows it for the RTT passes.
        let preview = self.ensure_preview(renderer);
        // A CHARACTER is its OWN subject: `ensure_preview` returned `None` for it, so its own mesh is
        // uploaded here and drawn at the same recentre as its skeleton. Same upload path as the prop
        // piece — the class only decides placement, so any conformable folder shows its mesh, with
        // no per-model wiring. Also uploaded before the grid borrow.
        let char_mesh = {
            let is_char = self
                .source
                .as_ref()
                .map(|s| matches!(s.class(), Some(AssetClass::Skin) | None))
                .unwrap_or(false);
            if is_char && self.wf.step() == "conform" {
                // Conform draws the LIVE CPU-skinned mesh so an authored bone offset (slider or the
                // gizmo drag) deforms it in view; it falls back to the rest mesh if there is nothing
                // to skin yet.
                self.ensure_skinned_mesh(renderer)
                    .or_else(|| self.ensure_source_mesh(renderer))
            } else if is_char {
                self.ensure_source_mesh(renderer)
            } else {
                None
            }
        };

        // Frame on the base body when previewing a piece (a prop skeleton has no extent). The
        // preview already recentred itself, so its offset here is zero.
        let (centre, radius, floor) = match preview.as_ref() {
            Some(p) => (Vec3::ZERO, p.radius, p.floor),
            None => self
                .source
                .as_ref()
                .and_then(|s| s.parsed.as_ref())
                .map(|p| (p.centre, p.radius, p.floor))
                .unwrap_or((Vec3::ZERO, 100.0, -100.0)),
        };
        // Remember what the view was framed on, so next frame's pan scales pixels to world units
        // at the same rate the camera does. Set before the `grid` borrow below.
        self.view_radius = radius;

        if let Some(grid) = self.grid.as_ref() {
            // The stage floor: a faint lattice on the XY ground plane at the asset's FEET, so the
            // perspective view reads as a stage instead of empty space. Everything is drawn
            // recentred, which puts the origin at the asset's WAIST — without this the eye has no
            // ground reference and the camera reads as if it were underground. Sized off the
            // asset so a weapon and a body both get a usable lattice.
            let ground = grid_segments_xy(radius * 0.25, radius * 2.5, floor);
            // Everything is drawn about the asset's centre: the cameras target the origin, and in
            // Z-up ground reckoning the origin is the asset's FEET.
            let recentre = Mat4::from_translation(-centre);
            // One camera PER VIEW, each from its own `Orbit` (independent pan + zoom); the ortho
            // views frame at `radius · zoom`, the perspective view at `radius · dist_scale · zoom`.
            let cameras: Vec<Camera> = (0..grid.views().len())
                .map(|i| grid.camera(i, self.orbits[i].ortho_radius(radius), &self.orbits[i].camera(radius)))
                .collect();
            // Skeleton rig-view: octahedral "diamond" bones + per-joint scaled balls (replaces the old
            // flat joint lines + orange frame-axis lines). The SELECTED joint (Conform stage) draws
            // amber with the transform gizmo on it; every non-selected joint draws a cyan ball sized to
            // its bone length. Recentred exactly like the mesh so they register; drawn in every panel,
            // but only the PERSP panel picks (see `gizmo_interact`).
            let parsed = self.source.as_ref().and_then(|s| s.parsed.as_ref());
            let sel = if self.wf.step() == "conform" {
                self.source
                    .as_ref()
                    .and_then(|s| s.rig.as_ref())
                    .filter(|r| r.focused)
                    .map(|r| r.sel)
            } else {
                None
            };
            let (bones, balls, sel_ball, gizmo_groups): (
                Segments,
                Segments,
                Segments,
                Vec<([f32; 4], Segments)>,
            ) = match (self.show_skeleton, parsed) {
                (true, Some(p)) => {
                    let min_r = (radius * BALL_MIN_FRAC).max(0.2);
                    let max_r = (radius * BALL_MAX_FRAC).max(min_r);
                    let radii = debug::joint_ball_radii(&p.parents, &p.globals, BALL_LEN_FRAC, min_r, max_r);
                    let bones = debug::bone_diamonds(recentre, &p.parents, &p.globals, BONE_WAIST_FRAC);
                    let mut balls = Segments::new();
                    for (i, g) in p.globals.iter().enumerate() {
                        if Some(i) == sel {
                            continue; // the focused joint draws amber below
                        }
                        let c = recentre.transform_point3(g.w_axis.truncate());
                        balls.extend(debug::wireframe(&Shape::Sphere { center: c, radius: radii[i] }));
                    }
                    // The FOCUSED joint: a larger amber ball (the grab handle) + the PAN gizmo. The
                    // gizmo is drawn ONLY in the ortho panels (see the draw loop) — Perspective grabs
                    // the ball for the spring-back deform test. The GIZMO MODE toggle picks the handle
                    // shape (translate now; rotate/scale are the seam for later pipeline stages).
                    let (sel_ball, gizmo_groups) = match sel.and_then(|s| p.globals.get(s).map(|g| (s, g))) {
                        Some((s, g)) => {
                            let c = recentre.transform_point3(g.w_axis.truncate());
                            let r = radii.get(s).copied().unwrap_or(0.5) * 1.4;
                            let ball = debug::wireframe(&Shape::Sphere { center: c, radius: r });
                            let size = (radius * GIZMO_ARROW_FRAC).max(1.0);
                            let lines = gizmo_segments(c, glam::Mat3::IDENTITY, self.gizmo_mode, size, None);
                            (ball, group_by_color(lines))
                        }
                        None => (Segments::new(), Vec::new()),
                    };
                    (bones, balls, sel_ball, gizmo_groups)
                }
                _ => (Segments::new(), Segments::new(), Segments::new(), Vec::new()),
            };
            // Collision overlay: the auto-fit capsules + leaf "joint ball" spheres wireframed over
            // the CHARACTER's posed rig, recentred exactly like its skeleton. Only in the character
            // case — a prop preview's frames are placed at a socket, not recentred, so its volumes
            // would land at the raw origin; a prop's fit-capsule-at-socket is a later concern.
            let collision: Vec<(Vec3, Vec3)> = if self.show_collision && preview.is_none() {
                self.source
                    .as_ref()
                    .and_then(|s| s.parsed.as_ref())
                    .map(|p| {
                        let mut segs = Vec::new();
                        for v in &p.collision {
                            if let Some(g) = p.globals.get(v.bone) {
                                segs.extend(debug::wireframe(&v.world(recentre * *g)));
                            }
                        }
                        segs
                    })
                    .unwrap_or_default()
            } else {
                Vec::new()
            };
            // Attach markers, once the stage is reached: every point in bronze, the selected one
            // larger and in rune-blue, exactly as the design distinguishes them. Recentred with the
            // skeleton they annotate, or they would float off it.
            let (marks, sel_marks) = self.attach_markers(radius);
            let shift = |s: Vec<(Vec3, Vec3)>| -> Vec<(Vec3, Vec3)> {
                s.into_iter().map(|(a, b)| (a - centre, b - centre)).collect()
            };
            let (marks, sel_marks) = (shift(marks), shift(sel_marks));
            grid.render_with(renderer, base_layer + 2.0, &cameras, |r, view| {
                r.set_scene(SceneLighting::default());
                // Ground in the PERSPECTIVE view only (index 0 of `EDITOR_QUADS`): the three
                // ortho views look straight down an axis, where the lattice collapses to a
                // single line or a moiré of edge-on rows and only obscures the measurement.
                // Depth-tested (`draw_lines`, not the overlay) so the asset stands ON it.
                if view == 0 && !ground.is_empty() {
                    r.draw_lines(&ground, GROUND);
                }
                // The character's own body, drawn at the SAME recentre as its skeleton so the two
                // register. Textured when it ships maps (neutral, so the real skin reads), else the
                // flat subject clay. Under the skeleton overlay, which reads through on top.
                if let Some(cm) = char_mesh {
                    cm.draw(r, recentre, SUBJECT_TINT);
                }
                // Skeleton rig-view: octahedral diamond bones + per-joint scaled balls, both cyan.
                if !bones.is_empty() {
                    r.draw_lines_overlay(&bones, JOINT);
                }
                if !balls.is_empty() {
                    r.draw_lines_overlay(&balls, JOINT);
                }
                // The FOCUSED joint's amber ball — its grab handle in every view.
                if !sel_ball.is_empty() {
                    r.draw_lines_overlay(&sel_ball, GIZMO_SEL);
                }
                // The pan gizmo on the focused joint — ORTHO panels only (view 0 = Perspective, where
                // you grab the ball for the spring-back deform test). Its two in-plane arrows are the
                // move handles for that view; the depth axis points at the camera.
                if view != 0 {
                    for (color, lines) in &gizmo_groups {
                        r.draw_lines_overlay(lines, *color);
                    }
                }
                if !collision.is_empty() {
                    r.draw_lines_overlay(&collision, COLLISION);
                }
                if !marks.is_empty() {
                    r.draw_lines_overlay(&marks, MARKER);
                }
                if !sel_marks.is_empty() {
                    r.draw_lines_overlay(&sel_marks, MARKER_SEL);
                }
                // Prop/garment: the base skeleton + the imported mesh placed at socket·fit, so the
                // user SEES the piece on the body while tuning the sliders (the whole point of the
                // fit stage). The mesh uses the SAME `attach_world` the bake does — no drift.
                if let Some(pv) = preview.as_ref() {
                    // The reference BODY first, then its skeleton, then the piece — so the fit is
                    // judged against a SHAPE (does the hair sit on the skull, is the blade in the
                    // grip) rather than against a stick figure.
                    // Textured when the source shipped maps (neutral, so the real skin reads),
                    // else the flat clay tints that keep body and piece separable.
                    if let Some(body) = pv.base_mesh {
                        body.draw(r, pv.base_world, BODY_TINT);
                    }
                    if !pv.base_joints.is_empty() {
                        r.draw_lines_overlay(&pv.base_joints, JOINT);
                    }
                    pv.mesh.draw(r, pv.world, PIECE_TINT);
                }
            });
        }

        // Opaque window backdrop at the base layer, under everything. Drawn AFTER the frame graph, or
        // the offscreen passes above would discard it; it fills the transparent gaps of the workbench
        // Column (the body margins around the holder and the rail).
        let screen = renderer.size();
        renderer.draw_ui_panel(
            Vec2::ZERO,
            screen,
            [0.03, 0.03, 0.04, 1.0],
            [0.03, 0.03, 0.04, 1.0],
            0.0,
            0.0,
            0.0,
            [0.0; 4],
            0.0,
        );

        // The HUD chrome — header, tab bar, the holder FRAME, the editor rail, footer — at `base+1`.
        // The four RTT views composite ABOVE it at `base+2`, so they land INSIDE the holder frame (a
        // HUD panel) at its inset, while the rail sits BESIDE the holder rather than under the
        // composite — the panels read as first-class regions of a tiled layout, not overlays.
        if let Some(white) = self.hud_white {
            renderer.set_layer(base_layer + 1.0);
            render_hud(renderer, &self.hud_commands, white, &[]);
            renderer.set_layer(base_layer);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The real source folder the whole pipeline is developed against. Every test that needs a
    /// genuine skeleton goes through this, and SKIPS when the content tree is absent — the same
    /// guard `flicker-content`'s own real-data tests use.
    fn real_source() -> Option<PathBuf> {
        let dir = PathBuf::from(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../content/source/PrismHumanBaseA"
        ));
        dir.exists().then_some(dir)
    }

    /// One fired workflow result, exactly as the footer / dialog buttons report it.
    fn fired(name: &str) -> ValueMap {
        ValueMap::new().with(name, true)
    }

    /// Load the shipped stringtable (en-us) into the process-wide table, so tests asserting
    /// chip labels / dialog copy read FINAL text. Safe across parallel test threads — every
    /// caller loads the same content, so the shared table never changes under an assertion.
    fn load_shipped_strings() {
        let strings = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../content/data/stringtable.json"
        ))
        .expect("stringtable reads");
        flicker::ui::strings::load_str(&strings, "en-us");
    }

    /// Park the wizard on `step` by walking the runtime FORWARD with a synthetic
    /// all-keys document — the test-only stand-in for the old direct `step` field
    /// write, for tests about a page's UI rather than its gates (the gates
    /// themselves are exercised by the tests that assert them, on the REAL doc).
    fn park(ed: &mut AssetPipeline, step: &str) {
        let doc = ValueMap::new()
            .with("source", true)
            .with("class", true)
            .with("rig", true)
            .with("attach", true)
            .with("fit", true);
        for _ in 0..ed.wf_def.steps.len() {
            if ed.wf.step() == step {
                return;
            }
            ed.wf.handle(&fired("wf_next"), &doc);
        }
        assert_eq!(ed.wf.step(), step, "`{step}` is ahead of here in the active workflow");
    }

    /// An editor with the real CHARACTER asset loaded, parsed and conformed — parked
    /// exactly where `open` lands it: the `conform` step of the character workflow.
    /// `open` dispatches the definition and runs ingest → parse → conform inline
    /// (exactly as clicking the Import Character card does), so the derived state
    /// exists just as `update` would leave it.
    fn at_conform() -> Option<AssetPipeline> {
        let dir = real_source()?;
        let mut ed = AssetPipeline::new();
        ed.pending_class = Some(AssetClass::Skin);
        ed.open(dir);
        assert!(ed.source.is_some(), "the real source folder scanned");
        assert_eq!(ed.wf.step(), "conform", "open lands on the rig view");
        Some(ed)
    }

    /// An editor with the real asset loaded and PARSED but NOT conformed — the state the removed
    /// `at_step(Analyze)` gave. Callers re-route the class (to a prop) or drive the picker from here,
    /// so the inline conform `open` runs is dropped to leave a clean parse on the rig step.
    fn parsed() -> Option<AssetPipeline> {
        let mut ed = at_conform()?;
        ed.source.as_mut().unwrap().rig = None;
        Some(ed)
    }

    /// The restored `Collision` overlay must have geometry to draw: a real character's auto-fit
    /// yields per-bone capsules (the "boxes") AND at least one leaf-bone sphere (the "joint balls"),
    /// every one indexing a real bone so the overlay can place it. Skips without the content tree.
    #[test]
    fn collision_overlay_has_capsules_and_joint_balls() {
        use flicker_mechanics::collision::Shape;
        let Some(ed) = parsed() else { return };
        let parsed = ed.source.as_ref().unwrap().parsed.as_ref().unwrap();
        assert!(!parsed.collision.is_empty(), "auto-fit produced collision volumes for the character");
        let capsules = parsed.collision.iter().filter(|v| matches!(v.shape, Shape::Capsule { .. })).count();
        let spheres = parsed.collision.iter().filter(|v| matches!(v.shape, Shape::Sphere { .. })).count();
        assert!(capsules > 0, "bones with children fit capsules (the collision boxes)");
        assert!(spheres > 0, "leaf bones (fingertips/toes/head end) fit spheres (the joint balls)");
        assert!(
            parsed.collision.iter().all(|v| v.bone < parsed.globals.len()),
            "every volume indexes a real bone, so `globals[v.bone]` places it"
        );
    }

    /// The canonical bone count is a canon constant, so it is asserted against the reference rig
    /// itself rather than trusted — a change to the reference fails HERE, not silently in a
    /// requirement that always reads red.
    #[test]
    fn reference_rig_still_has_the_canonical_bone_count() {
        let path = default_reference();
        if !flicker_content::package::file_exists(&path) {
            eprintln!("skipping: {} not present", path.display());
            return;
        }
        let raw = flicker_content::package::read_text(&path).expect("read the reference rig");
        let json: serde_json::Value = serde_json::from_str(&raw).expect("parse the reference rig");
        let bones = json["skeleton"]["bones"].as_array().expect("skeleton.bones").len();
        assert_eq!(
            bones, REFERENCE_BONES,
            "the reference rig moved to {bones} bones — update REFERENCE_BONES and sweep the canon"
        );
    }

    /// CONFORM against the real source: every bone lands in exactly one provenance bucket, the
    /// buckets sum to the whole skeleton, and the inferred set is the one the reports name.
    /// This is the stage's contract — the bone map's colours have no second source.
    #[test]
    fn conform_of_the_real_source_classifies_every_bone() {
        let Some(ed) = at_conform() else {
            eprintln!("skipping: no content tree");
            return;
        };
        let src = ed.source.as_ref().unwrap();
        let rig = src.rig.as_ref().expect("conform produced a rig");
        let parsed = src.parsed.as_ref().unwrap();

        // 65, not 66: `root` is synthesized by the bake, not by conform.
        assert_eq!(parsed.bones(), CONFORMED_BONES, "conform reaches the canonical bone count");
        assert_eq!(rig.map.len(), parsed.bones(), "one provenance per bone, no gaps");
        let (ok, review, auto) = rig.counts();
        assert_eq!(ok + review + auto, parsed.bones(), "the buckets partition the skeleton");
        assert_eq!(
            auto,
            rig.out.infer.added.len(),
            "auto is exactly what infer added — not a recount"
        );
        assert!(review > 0, "the hip/shoulder/ankle derives flagged joints for review");
        assert!(ok > 0, "source bones survived the rename");

        // The reports are the ONE source: a bone infer added must not also read as review.
        for (i, b) in parsed.model.bones.iter().enumerate() {
            if rig.out.infer.added.iter().any(|a| a == &b.name) {
                assert_eq!(rig.map[i], MapState::Auto, "{} is inferred", b.name);
            }
        }
    }

    /// An authored offset moves the derived skeleton — and a zero offset reproduces the conform
    /// exactly, which is what makes "Reset bone" a real undo rather than an approximation.
    #[test]
    fn authored_offsets_move_the_skeleton_and_reset_restores_it() {
        let Some(mut ed) = at_conform() else {
            eprintln!("skipping: no content tree");
            return;
        };
        let src = ed.source.as_mut().unwrap();
        let rig = src.rig.as_mut().unwrap();
        // Pick a bone with children so the offset has to propagate down the chain.
        rig.sel = src
            .parsed
            .as_ref()
            .unwrap()
            .bone_index("spine_01")
            .expect("the conformed rig has spine_01");
        let sel = rig.sel;
        let before = src.parsed.as_ref().unwrap().globals.clone();

        rig.offsets[sel] = BoneOffset { t: [0.0, 0.0, 7.0], roll: 0.0 };
        let offsets = rig.offsets.clone();
        src.parsed.as_mut().unwrap().rebuild(&offsets);
        let after = src.parsed.as_ref().unwrap().globals.clone();

        assert_ne!(
            before[sel].w_axis, after[sel].w_axis,
            "the edited bone moved"
        );
        let head = src.parsed.as_ref().unwrap().bone_index("head").unwrap();
        assert_ne!(before[head].w_axis, after[head].w_axis, "the offset propagated to children");

        // Reset → identical frames, bit for bit.
        let src = ed.source.as_mut().unwrap();
        src.rig.as_mut().unwrap().offsets[sel] = BoneOffset::default();
        let offsets = src.rig.as_ref().unwrap().offsets.clone();
        src.parsed.as_mut().unwrap().rebuild(&offsets);
        assert_eq!(
            src.parsed.as_ref().unwrap().globals, before,
            "zeroing the offset restores the conform result exactly"
        );
    }

    /// The bone map pages rather than truncating: every one of the 66 bones is reachable, and
    /// the window always contains the selection.
    #[test]
    fn the_bone_map_pages_over_the_whole_skeleton() {
        let Some(mut ed) = at_conform() else {
            eprintln!("skipping: no content tree");
            return;
        };
        let total = ed.source.as_ref().unwrap().rig.as_ref().unwrap().map.len();

        // Page to the end with the same action the button fires. The window must actually
        // advance — a window that snapped back to the selection would loop here forever.
        let mut seen = 0;
        for _ in 0..total {
            ed.apply_stage_results(&ValueMap::new().with("bone_next", true));
            let rig = ed.source.as_ref().unwrap().rig.as_ref().unwrap();
            seen = seen.max(rig.window + BONE_ROWS);
            assert!(rig.sel >= rig.window && rig.sel < rig.window + BONE_ROWS, "cursor stays visible");
            if rig.sel + 1 >= total {
                break;
            }
        }
        assert!(seen >= total, "paging reached the last bone ({seen} of {total})");

        // And back again, without overshooting the top.
        for _ in 0..total {
            ed.apply_stage_results(&ValueMap::new().with("bone_prev", true));
        }
        let rig = ed.source.as_ref().unwrap().rig.as_ref().unwrap();
        assert_eq!((rig.sel, rig.window), (0, 0), "paging back lands on the first bone");

        // Selecting a row off-window scrolls it into view.
        let rig = ed.source.as_mut().unwrap().rig.as_mut().unwrap();
        rig.sel = total - 1;
        rig.window = 0;
        AssetPipeline::scroll_to_selection(rig);
        assert!(rig.sel >= rig.window && rig.sel < rig.window + BONE_ROWS, "selection is visible");
    }

    /// ATTACH: a point sits at its parent bone's conformed frame plus the authored offset, and
    /// all six resolve once the rig carries canonical names.
    #[test]
    fn attach_points_track_their_parent_bone_and_offset() {
        let Some(mut ed) = at_conform() else {
            eprintln!("skipping: no content tree");
            return;
        };
        park(&mut ed, "attach");
        let n = ed.source.as_ref().unwrap().attach.len();
        assert_eq!(n, 6, "the design's six points");
        assert!(
            (0..n).all(|i| ed.attach_resolved(i)),
            "every parent bone exists in the conformed rig: {:?}",
            (0..n).map(|i| ed.attach_resolved(i)).collect::<Vec<_>>()
        );

        // Selecting a point then dragging its X slider moves exactly that point.
        ed.apply_stage_results(&ValueMap::new().with("att_sel_2", true));
        assert_eq!(ed.source.as_ref().unwrap().attach_sel, 2);
        let before = ed.attach_world(2).expect("resolves");
        ed.apply_stage_results(&ValueMap::new().with("att_x", 5.0));
        let after = ed.attach_world(2).expect("still resolves");
        assert!((after.x - before.x - 5.0).abs() < 1e-4, "{before} → {after}");
        assert_eq!(ed.attach_world(3).unwrap(), ed.attach_world(3).unwrap(), "others unmoved");

        // Markers only exist on the stages that authored them. Back is clean here — the
        // direct `apply_stage_results` call above bypassed `update`, so nothing armed the
        // discard guard and the runtime steps straight back.
        assert!(!ed.attach_markers(100.0).0.is_empty(), "markers drawn on the Attach stage");
        ed.apply_workflow_results(&fired("wf_back"));
        assert_eq!(ed.wf.step(), "conform");
        assert!(ed.attach_markers(100.0).0.is_empty(), "and not on the rig stage before it");
    }

    /// REVIEW: every requirement is computed from real state. With the real asset conformed they
    /// all pass; with nothing loaded there is nothing to claim.
    #[test]
    fn review_requirements_read_the_real_state() {
        let empty = AssetPipeline::new();
        assert!(empty.requirements().is_empty(), "no asset → no claims");

        let Some(mut ed) = at_conform() else {
            eprintln!("skipping: no content tree");
            return;
        };
        park(&mut ed, "review");
        let reqs = ed.requirements();
        assert_eq!(reqs.len(), REQUIREMENT_ROWS, "the tree declares this many rows");
        for (ok, text) in &reqs {
            assert!(ok, "requirement failed on the reference asset: {text}");
        }

        // A requirement is a real gate: break one and Commit must go dark.
        let m = ed.hud_model();
        assert!(m.is_on("commit_enabled"));
        ed.source.as_mut().unwrap().textures = 0;
        assert!(!ed.hud_model().is_on("commit_enabled"), "a failed check blocks commit");
    }

    /// The wizard cannot be walked past a stage whose declared needs are missing: the
    /// `attach` step needs the `rig` document key, which is what stops Attach resolving
    /// against an un-conformed skeleton — asserted on the REAL document, not a synthetic one.
    #[test]
    fn stage_gates_require_their_input() {
        let Some(mut ed) = parsed() else {
            eprintln!("skipping: no content tree");
            return;
        };
        assert!(!ed.wf.can_next(&ed.wf_doc()), "Conform has not produced a rig yet");
        ed.apply_workflow_results(&fired("wf_next"));
        assert_eq!(ed.wf.step(), "conform", "the refused advance holds the page");
        ed.conform();
        assert!(ed.wf.can_next(&ed.wf_doc()), "with a rig it can");
    }

    /// A non-character is ROUTED, not force-conformed: declaring the asset a Prop makes
    /// Conform a no-op — no rig, and crucially no invented "no skeleton" failure — and the
    /// stage states honestly which class it is and that the in-app bake is not wired yet.
    /// This is the fix for "the import expects a specific thing": it now respects the class.
    #[test]
    fn a_prop_is_routed_not_conform_failed() {
        let Some(dir) = real_source() else {
            eprintln!("skipping: no content tree");
            return;
        };
        load_shipped_strings(); // the badge / inspector copy asserted below is token-resolved
        let mut ed = AssetPipeline::new();
        // The Import Prop card's declaration — `open` dispatches the prop workflow with it.
        ed.pending_class = Some(AssetClass::Prop);
        ed.open(dir);
        assert_eq!(ed.wf.step(), "conform", "a prop lands on its Mount page");
        let src = ed.source.as_ref().unwrap();
        assert!(src.rig.is_none(), "the character conform path must not run on a prop");
        assert!(src.error.is_none(), "and it must NOT invent a skeleton failure");
        // A prop SKIPS the character-only pages and walks on to commit its mesh directly —
        // its definition's next stop is Review, whose `fit` need the mount binding meets.
        assert!(ed.wf.can_next(&ed.wf_doc()), "a prop walks past the character-only conform");
        assert_eq!(ed.inspector_badge(), "STATIC", "a non-clothing prop bakes as a static mesh");
        let lines = ed.inspector_lines();
        assert!(
            lines.iter().any(|l| l.to_ascii_lowercase().contains("prop")),
            "the stage names the detected class: {lines:?}"
        );
        assert!(
            lines.iter().any(|l| l.to_ascii_lowercase().contains("paperdoll")),
            "and says the socket/fit are authored in the paperdoll: {lines:?}"
        );
    }

    /// Commit ROUTES by class: a Prop writes a STATIC-prop rig (empty skeleton, retarget:false),
    /// not a conformed character — the wizard's prop bake path exercised end to end against a
    /// scratch dir (the character source stands in for a prop mesh; the class override is what
    /// selects the bake, which is the routing under test).
    #[test]
    fn commit_routes_a_prop_to_the_static_bake() {
        let Some(mut ed) = parsed() else {
            eprintln!("skipping: no content tree");
            return;
        };
        {
            let s = ed.source.as_mut().unwrap();
            s.class = Some(AssetClass::Prop);
            s.prop = PropKind::Weapon;
        }
        let scratch = std::env::temp_dir().join("flicker_assetpipeline_prop_commit");
        let _ = std::fs::remove_dir_all(&scratch);
        ed.commit_to(&scratch);

        let src = ed.source.as_ref().unwrap();
        assert!(src.error.is_none(), "the prop commit succeeds: {:?}", src.error);
        let out = src.committed.clone().expect("a committed path is recorded");
        let text = flicker_content::package::read_text(&out).expect("the prop rig was written");
        assert!(text.contains("\"bones\":[]"), "a prop bakes an EMPTY skeleton: {out:?}");
        assert!(text.contains("\"retarget\":false"), "a prop is retarget:false");

        // And it SHIPS ITS TEXTURES: the wizard hands the bake the source mesh, so the vendor's maps
        // are copied beside the rig under the content standard's names and referenced by the
        // material — a prop that arrives as a lone `.json` renders untextured.
        let name = ed.source.as_ref().unwrap().asset_name().to_string();
        let dir = out.parent().expect("the rig sits in the asset's folder");
        let rig: flicker_skeletal::format::RigFile =
            serde_json::from_str(&text).expect("the prop rig parses");
        let m = rig.mesh.materials.first().expect("the prop has a material");
        assert_eq!(m.base_color, format!("{name}_BaseColor.png"), "albedo wired into the material");
        assert!(dir.join(&m.base_color).exists(), "and copied beside the rig");
        for map in [&m.normal, &m.roughness, &m.metalness] {
            assert!(!map.is_empty(), "every source map the standard has a slot for is wired");
            assert!(dir.join(map).exists(), "{map} copied beside the rig");
        }
        let _ = std::fs::remove_dir_all(&scratch);
    }

    /// MOST source folders hold SEVERAL riggable meshes — a weapon set is four or five pieces, an
    /// outfit is tops/pants/gloves/shoes. Such a folder must offer a PICKER, not be refused: it lands
    /// on the rig view with the first pre-selected, shows the INLINE picker, and picking a different
    /// piece re-points the import AND drops everything derived from the previous one.
    #[test]
    fn a_multi_mesh_folder_offers_a_picker() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../content/source/PrismWeaps/MuseEpicSet");
        if !dir.exists() {
            eprintln!("skipping: no PrismWeaps source");
            return;
        }
        let mut ed = AssetPipeline::new();
        ed.pending_class = Some(AssetClass::Prop); // the Import Accessory / Prop card's declaration
        ed.open(dir);
        {
            let src = ed.source.as_ref().expect("the folder opened");
            assert!(src.candidates.len() > 1, "a weapon set holds several meshes");
            assert!(src.error.is_none(), "several meshes is a CHOICE, not an error: {:?}", src.error);
            assert_eq!(src.fbx, src.candidates[0], "the first is pre-selected — never stuck");
        }
        assert_eq!(ed.wf.step(), "conform", "lands on the rig view, first piece ready");
        assert!(ed.hud_model().is_on("on_pick"), "the inline picker is shown for a choice");
        assert!(ed.source.as_ref().unwrap().parsed.is_some(), "the first pick is parsed on open");

        // Choose the second — the stale parse must be dropped so nothing derived carries forward.
        ed.apply_stage_results(&ValueMap::new().with("pick_sel_1", true));
        let src = ed.source.as_ref().unwrap();
        assert_eq!(src.candidate_sel, 1);
        assert_eq!(src.fbx, src.candidates[1], "the import now points at the second mesh");
        assert!(src.parsed.is_none(), "the previous mesh's parse was dropped");
        assert!(src.report.is_none() && src.rig.is_none(), "and everything derived from it");
    }

    /// THE multi-piece LOOP: once a piece is committed, "import next piece" returns to the rig view
    /// with the folder + its piece list intact and everything derived from the finished piece
    /// dropped — so a weapon set or an outfit is walked one piece at a time, formally, without
    /// leaving the scene. The inline picker is right there to choose the next piece.
    #[test]
    fn committing_a_piece_offers_the_loop_back_to_the_picker() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../content/source/PrismWeaps/MuseEpicSet");
        if !dir.exists() {
            eprintln!("skipping: no PrismWeaps source");
            return;
        }
        let mut ed = AssetPipeline::new();
        ed.pending_class = Some(AssetClass::Prop);
        ed.open(dir);
        let scratch = std::env::temp_dir().join("flicker_assetpipeline_next_piece");
        let _ = std::fs::remove_dir_all(&scratch);
        // Forward through the REAL gate: the prop's mount binding meets Review's `fit` need.
        ed.wf_advance();
        assert_eq!(ed.wf.step(), "review");
        ed.commit_to(&scratch);
        assert!(
            ed.source.as_ref().unwrap().committed.is_some(),
            "the piece baked: {:?}",
            ed.source.as_ref().unwrap().error
        );
        assert!(ed.hud_model().is_on("has_committed"), "the loop-back is offered");

        let n = ed.source.as_ref().unwrap().candidates.len();
        ed.start_next_piece();
        assert_eq!(ed.wf.step(), "conform", "back on the rig view for the next piece");
        let src = ed.source.as_ref().unwrap();
        assert_eq!(src.candidates.len(), n, "the folder and its piece list are kept");
        assert!(
            src.parsed.is_none() && src.rig.is_none() && src.committed.is_none(),
            "the finished piece's state is dropped so the next starts clean"
        );
        assert!(ed.hud_model().is_on("on_pick"), "the inline picker is showing again");
        let _ = std::fs::remove_dir_all(&scratch);
    }

    /// BACK walks the rail, and on the first page (Workflow) — where there is no earlier step — it
    /// clears the open folder instead of being inert, so a wrongly-chosen folder is recoverable.
    #[test]
    fn back_walks_the_rail_and_clears_the_folder_at_the_top() {
        let Some(mut ed) = at_conform() else {
            eprintln!("skipping: no content tree");
            return;
        };
        // From the rig view, Back returns to the Workflow page with the folder still open.
        ed.apply_workflow_results(&fired("wf_back"));
        assert_eq!(ed.wf.step(), "task", "Back from the rig view returns to the workflow page");
        assert!(ed.source.is_some(), "which leaves the folder open");
        assert!(ed.hud_model().is_on("back_enabled"), "Back stays LIVE with a folder open — it clears it");

        // Back again on the Workflow page (the first, with no earlier stop) clears the open folder.
        ed.apply_workflow_results(&fired("wf_back"));
        assert!(ed.source.is_none(), "Back on the workflow page clears the open folder");
        assert!(!ed.hud_model().is_on("back_enabled"), "and then there is nothing to go back from");
    }

    /// The FIT stage is the prop/garment's human-in-the-loop mount authoring: for a non-character
    /// the Attach step shows the fit subtree (not the six character points), and picking a socket
    /// row + dragging a fit slider lands in `src.fit` — which Commit then bakes. This is the whole
    /// point of the tool: the human places and verifies, the bake honours it.
    #[test]
    fn fit_stage_authors_the_prop_mount() {
        let Some(dir) = real_source() else {
            eprintln!("skipping: no content tree");
            return;
        };
        load_shipped_strings(); // step_title asserted below is token-resolved
        let mut ed = AssetPipeline::new();
        // The Import Prop card's declaration; `open` dispatches the prop workflow, whose
        // rig page IS Conform under the Mount role — not a separate later stage.
        ed.pending_class = Some(AssetClass::Prop);
        ed.open(dir);
        assert_eq!(ed.wf.step(), "conform");

        // Conform DISPATCHES: a prop gets the mount panel, and NOT the character bone map that
        // used to leave this page empty for it.
        let m = ed.hud_model();
        assert_eq!(ed.conform_role(), ConformRole::Mount, "a prop conforms by mounting");
        assert!(m.is_on("on_conform_mount"), "a prop authors its mount ON the conform page");
        assert!(!m.is_on("on_conform_skeleton"), "and NOT the character bone map");
        assert!(!m.is_on("on_conform_clip"), "and NOT the animation placeholder");
        // The page names itself for the role, so the rail/footer never say "Conform Rig" here.
        assert_eq!(m.text("step_title"), Some("Mount"));

        // Pick a socket row and drag the X-offset + scale sliders, exactly as the walker reports.
        let window = ed.source.as_ref().unwrap().fit_window;
        ed.apply_stage_results(
            &ValueMap::new()
                .with("sock_sel_2", true)
                .with("fit_ox", 3.5)
                .with("fit_rz", 45.0)
                .with("fit_sy", 2.0)
                .with("fit_scale", 1.5),
        );
        let fit = ed.source.as_ref().unwrap().fit;
        assert_eq!(fit.socket, window + 2, "the picked socket row is now the mount");
        assert!((fit.offset[0] - 3.5).abs() < 1e-4, "the offset slider authored the fit");
        assert!((fit.rot[2] - 45.0).abs() < 1e-4, "the rotation slider authored the fit");
        // Per-axis scale RESHAPES (only the dragged axis moves) and scale-all is a SEPARATE
        // multiplier — the paperdoll gadget's pair. Conflating them would silently rescale the
        // other two axes the moment the user touched one.
        assert!((fit.scale[1] - 2.0).abs() < 1e-4, "the Y scale slider authored that axis");
        assert!((fit.scale[0] - 1.0).abs() < 1e-4, "and left X alone");
        assert!((fit.scale[2] - 1.0).abs() < 1e-4, "and left Z alone");
        assert!((fit.uniform - 1.5).abs() < 1e-4, "scale-all rides `fit_scale`");

        // The whole point of widening: both reach the BAKED rig, because the format already
        // carried `scale` × `uniform` and `attach_world` already applied it.
        let baked = Fit {
            socket: fit.socket_name().to_string(),
            offset: fit.offset,
            rot_deg: fit.rot,
            scale: fit.scale,
            uniform: fit.uniform,
        }
        .to_attach();
        assert!((baked.scale[1] - 2.0).abs() < 1e-4, "per-axis scale survives to the format");
        assert!((baked.uniform - 1.5).abs() < 1e-4, "scale-all survives to the format");

        // And that authored socket is a REAL bone name the bake can resolve against the base.
        assert!(
            SOCKETS.get(fit.socket).is_some(),
            "the mount indexes the socket table"
        );
    }

    /// GEOMETRY GUARD — no HUD draw command may escape the screen horizontally at the real
    /// display sizes. The binding tests prove the tree walks; this proves it FITS. A mis-sized
    /// button or a text slot placed past a panel edge (the "corrupted layout" this HUD carried,
    /// from `size` doubling as a Row's width) surfaces here as a rect or glyph beyond the right
    /// edge — caught in CI instead of by eye in the window, which the harness cannot open.
    #[test]
    fn hud_never_overflows_the_screen_width() {
        // The REAL tree, through the scene's own single seam — so this gate inspects
        // exactly what the app draws (8634C200: a gate that builds its own unexpanded
        // tree certifies the drift it exists to catch).
        let tree = AssetPipeline::new().build_tree(Vec2::new(1600.0, 900.0));
        let styles = load_styles(HUD_UI_ELEMENTS);

        // Vocabulary gate, same spirit as the overflow check below: an unknown kind draws
        // NOTHING, so a name left behind by a rename is invisible in the window this
        // harness cannot open.
        assert!(
            flicker::ui::unknown_kinds(&tree).is_empty(),
            "clayworks_bench names unknown kinds: {:?}",
            flicker::ui::unknown_kinds(&tree)
        );
        // The strings gate (S10): every display literal is a `$token`.
        assert!(
            flicker::ui::raw_display_literals(&tree).is_empty(),
            "clayworks_bench ships raw display literals: {:?}",
            flicker::ui::raw_display_literals(&tree)
        );
        // The MODEL-CHANNEL strings gate (S10's blind side): display copy published from
        // Rust into the Model bypasses the tree gate above, so the crate self-gates its
        // OWN source — every `.set`/`.with` value must be a resolved `$token`, a data
        // shape, or carry an explicit `strings-gate-exempt` reason.
        let flags = strings::raw_model_publish_literals(include_str!("lib.rs"));
        assert!(flags.is_empty(), "raw display copy published into the Model: {flags:?}");
        // The workflow drift gate: every step of BOTH shipped definitions has a node in
        // the REAL tree gating on its surface key — a definition/tree mismatch is a build
        // failure here, not a step that flips its surface and renders a blank page.
        let mut gate_ed = AssetPipeline::new();
        assert!(
            gate_ed.wf.ungated_steps(&tree).is_empty(),
            "character workflow steps with no visible_bind in the tree: {:?}",
            gate_ed.wf.ungated_steps(&tree)
        );
        gate_ed.dispatch_workflow(Some(AssetClass::Prop));
        assert!(
            gate_ed.wf.ungated_steps(&tree).is_empty(),
            "prop workflow steps with no visible_bind in the tree: {:?}",
            gate_ed.wf.ungated_steps(&tree)
        );

        // The user's 1600×900 fullscreen (settings.json) and the 1280×720 windowed default.
        const SIZES: [Vec2; 2] = [Vec2::new(1600.0, 900.0), Vec2::new(1280.0, 720.0)];
        let tol = 1.5_f32;
        let check = |model: &ValueMap, screen: Vec2, step: &str| {
            let snap = UiInput {
                mouse: Vec2::new(-100.0, -100.0),
                clicked: false,
                down: false,
                screen,
                typed: String::new(),
                backspace: false,
                wheel: 0.0,
            };
            let mut state = UiState::new();
            let frame = run_ui(&tree, model, &styles, &snap, &mut state);
            for cmd in &frame.commands {
                let (x, right, y) = match *cmd {
                    HudCommand::Rect { x, y, w, .. } | HudCommand::Sprite { x, y, w, .. } => {
                        (x, x + w, y)
                    }
                    // A right-aligned label's `x` is already its right edge (it renders leftward);
                    // a left/centre one's `x` is its start. Either way `x` must be on-screen.
                    HudCommand::Text { x, y, .. } => (x, x, y),
                    _ => continue,
                };
                assert!(
                    x >= -tol && right <= screen.x + tol,
                    "{step} @ {screen:?}: draw x={x}..{right} escapes width {}",
                    screen.x
                );
                assert!(y >= -tol, "{step} @ {screen:?}: draw y={y} is above the top edge");
            }
        };

        // The Workflow page is the FIRST screen shown and needs no content — always exercise the top
        // bar + footer here (the empty-state the user first hit).
        for screen in SIZES {
            check(&AssetPipeline::new().hud_model(), screen, "task");
        }

        // The later stages need a parsed source; skip cleanly when the content tree is absent.
        let Some(mut ed) = parsed() else {
            eprintln!("skipping stage bounds: no content tree");
            return;
        };
        // Every stop of the character workflow: Back to Task first (the folder stays open),
        // then walk forward, conforming when the rig view is reached.
        ed.apply_workflow_results(&fired("wf_back"));
        for step in ["task", "conform", "attach", "review"] {
            park(&mut ed, step);
            if step == "conform" {
                ed.conform();
            }
            for screen in SIZES {
                check(&ed.hud_model(), screen, step);
            }
        }

        // A non-character's rig page is Conform under the Mount role (socket picker +
        // offset/rotation/scale sliders) in place of the character bone map — verify THAT
        // subtree also fits the screen.
        ed.source.as_mut().unwrap().class = Some(AssetClass::Prop);
        ed.apply_workflow_results(&fired("wf_back"));
        ed.apply_workflow_results(&fired("wf_back"));
        assert_eq!(ed.wf.step(), "conform");
        for screen in SIZES {
            check(&ed.hud_model(), screen, "conform");
        }
    }

    /// Every stage's controls must walk against the REAL Lua tree and the REAL json — this is
    /// what turns a renamed bind or a missing style path into a build failure instead of an
    /// empty panel found in the window.
    #[test]
    fn every_stage_subtree_walks_and_binds() {
        let Some(mut ed) = at_conform() else {
            eprintln!("skipping: no content tree");
            return;
        };
        load_shipped_strings(); // the per-step needles are token-resolved copy now
        // The REAL tree, through the scene's own single seam — so this gate inspects
        // exactly what the app draws (8634C200: a gate that builds its own unexpanded
        // tree certifies the drift it exists to catch).
        let tree = AssetPipeline::new().build_tree(Vec2::new(1600.0, 900.0));
        let styles = load_styles(HUD_UI_ELEMENTS);

        // One representative bound string per stage — if the stage's subtree failed to build or
        // its binding was renamed, the string never reaches the draw commands.
        for (step, needle) in [
            ("conform", "Internal rig"),
            ("attach", "Grip"),
            ("review", "skeleton conforms"),
        ] {
            park(&mut ed, step);
            let model = ed.hud_model();
            let snap = UiInput {
                mouse: Vec2::new(-100.0, -100.0),
                clicked: false,
                down: false,
                screen: Vec2::new(1520.0, 980.0),
                typed: String::new(),
                backspace: false,
                wheel: 0.0,
            };
            let mut state = UiState::new();
            let frame = run_ui(&tree, &model, &styles, &snap, &mut state);
            let drew = frame.commands.iter().any(|c| {
                matches!(c, HudCommand::Text { text, .. } if text.contains(needle))
            });
            assert!(drew, "{step:?}: no draw command carried {needle:?}");
        }
    }

    /// COMMIT writes a rig the engine's own loader accepts, carrying the authored offsets and
    /// the bake's synthesized root. Written to a scratch dir — the live content tree is Aaron's,
    /// and a test that rewrote a shipped character would be a destructive one.
    #[test]
    fn commit_writes_a_loadable_rig_carrying_the_authored_offsets() {
        let Some(mut ed) = at_conform() else {
            eprintln!("skipping: no content tree");
            return;
        };
        // Author a distinctive offset so the written file can be told from a plain bake.
        let sel = {
            let src = ed.source.as_mut().unwrap();
            let i = src.parsed.as_ref().unwrap().bone_index("head").unwrap();
            let rig = src.rig.as_mut().unwrap();
            rig.sel = i;
            rig.offsets[i] = BoneOffset { t: [0.0, 0.0, 3.5], roll: 0.0 };
            i
        };
        let baseline = ed.source.as_ref().unwrap().parsed.as_ref().unwrap().model.bones[sel]
            .translation[2];

        let out_root = std::env::temp_dir().join("flicker_assetpipeline_commit");
        let _ = std::fs::remove_dir_all(&out_root);
        park(&mut ed, "review");
        ed.commit_to(&out_root);

        let src = ed.source.as_ref().unwrap();
        assert!(src.error.is_none(), "commit reported: {:?}", src.error);
        let written = src.committed.as_ref().expect("commit recorded where it wrote");
        assert!(flicker_content::package::file_exists(written), "{} was written", written.display());

        // Round-trip through the ENGINE's loader, not a bespoke parse — if the bake drifted from
        // what the runtime accepts, this is where it shows.
        let raw = flicker_content::package::read_text(written).unwrap();
        let json: serde_json::Value = serde_json::from_str(&raw).expect("valid rig json");
        let bones = json["skeleton"]["bones"].as_array().expect("skeleton.bones");
        assert_eq!(bones.len(), REFERENCE_BONES, "the bake synthesized the root");
        assert_eq!(bones[0]["name"], "root", "root is bone 0");

        // The authored offset is IN the file: the working model is untouched, the bake carries it.
        assert_eq!(
            ed.source.as_ref().unwrap().parsed.as_ref().unwrap().model.bones[sel].translation[2],
            baseline,
            "the working model stays the conform baseline — offsets remain reversible"
        );
        let head = bones
            .iter()
            .find(|b| b["name"] == "head")
            .expect("head survived the bake");
        // `local` is a column-major 4x4; the translation is the last column's first three.
        let local = head["local"].as_array().expect("local matrix");
        let tz = local[14].as_f64().expect("t.z") as f32;
        assert!(
            (tz - (baseline + 3.5)).abs() < 1e-3,
            "the authored +3.5 is baked in: {tz} vs {}",
            baseline + 3.5
        );

        let _ = std::fs::remove_dir_all(&out_root);
    }

    /// The bone map's colours resolve to REAL rgba through the shared palette — a typo'd or
    /// deleted `assetpipeline.map.*` key would otherwise show as default ink and read as fine.
    #[test]
    fn bone_map_colours_resolve_against_the_shared_palette() {
        let styles = load_styles(HUD_UI_ELEMENTS);
        for state in [MapState::Ok, MapState::Review, MapState::Auto] {
            let mut node = &styles;
            for part in state.color().split('.') {
                node = &node[part];
            }
            let rgba = node.as_array().unwrap_or_else(|| {
                panic!("{} did not resolve to an rgba array: {node:?}", state.color())
            });
            assert_eq!(rgba.len(), 4, "{} is rgba", state.color());
        }
        // The Review stage's failure colour rides the same block.
        assert!(styles["assetpipeline"]["map"]["fail"].is_array());
    }

    /// The real HUD script + the real `ui_elements.json` must load and walk together —
    /// this is what makes a missing `UI.assetpipeline` key or a renamed bind a BUILD
    /// failure rather than an empty panel discovered in the window.
    #[test]
    fn hud_tree_walks_against_the_shared_ui_elements() {
        // The HUD's display copy is `$token`s now (S10 strings gate); load the
        // shipped table so the walked commands carry the resolved en-us text.
        load_shipped_strings();
        let styles = load_styles(HUD_UI_ELEMENTS);
        let mut ed = AssetPipeline::new();
        let tree = ed.build_tree(Vec2::new(1600.0, 900.0));
        let model = ed.hud_model();
        let snap = UiInput {
            mouse: Vec2::new(-100.0, -100.0), // parked off-panel: no hover, no click
            clicked: false,
            down: false,
            screen: Vec2::new(1520.0, 980.0),
            typed: String::new(),
            backspace: false,
            wheel: 0.0,
        };
        let mut state = UiState::new();
        let frame = run_ui(&tree, &model, &styles, &snap, &mut state);
        assert!(
            !frame.commands.is_empty(),
            "the walker drew nothing — the tree resolved to an empty page"
        );
        // The footer's step hint is a bound string, so seeing it proves the Model→tree binding
        // actually resolved rather than silently rendering blanks. The entry page is Task now, so
        // its hint is what the first frame draws.
        let drew_hint = frame.commands.iter().any(|c| {
            matches!(c, HudCommand::Text { text, .. } if text.contains("Choose the kind of asset"))
        });
        assert!(drew_hint, "the bound step hint did not reach the draw commands");

        // The Task overlay is now built from the STANDARD templates: the `frame` gives the "Import"
        // title bar (its `n` region) and glowing CORNER RUNES, and each DS `card` gives a workflow-noun
        // title. Prove those reach the draw commands so a regression in the template wiring fails HERE.
        let text_is = |needle: &str| {
            frame.commands.iter().any(|c| matches!(c, HudCommand::Text { text, .. } if text == needle))
        };
        assert!(text_is("Import"), "the window title bar did not draw");
        assert!(text_is("Character"), "the first workflow card title did not draw");
        // The rail chips ride the workflow runtime's published binds all the way to DRAW:
        // the character definition's four pre-localized titles are on the strip.
        for chip in ["Workflow", "Rig", "Attach", "Review"] {
            assert!(text_is(chip), "rail chip {chip:?} did not draw");
        }
        assert!(
            frame
                .commands
                .iter()
                .any(|c| matches!(c, HudCommand::Text { font: flicker::script::FontRole::Rune, .. })),
            "the window's corner runes did not draw"
        );
        // The card's italic flavour WRAPS: it reaches the draw commands carrying a wrap width, so a
        // long line breaks to the card instead of running off it.
        let wrapped_flavour = frame.commands.iter().any(|c| {
            matches!(c, HudCommand::Text { text, wrap: Some(w), .. }
                if text.contains("A soul given form") && *w > 0.0)
        });
        assert!(wrapped_flavour, "the card flavour did not draw as wrapped text with a bound width");
    }

    /// The right-drag pan slides the LOOK-AT POINT across the view plane so the content tracks the
    /// cursor. Pure math — no GPU, no content.
    #[test]
    fn right_drag_pan_tracks_the_cursor_without_rotating_the_view() {
        // Look down +X at the origin (yaw 0, pitch 0): screen-right is +Y, screen-up is +Z.
        let mut o = Orbit { yaw: 0.0, pitch: 0.0, dist_scale: 2.4, zoom: 1.0, pan: Vec3::ZERO };
        let (radius, vh) = (100.0_f32, 800.0_f32);
        let before = o.camera(radius);
        let persp = o.camera(radius);

        // Cursor RIGHT: the content follows it, so the target slides the OTHER way along
        // screen-right (+Y). Getting this backwards is the classic inverted-pan bug.
        o.pan_by_view(Vec2::new(10.0, 0.0), &persp, vh);
        assert!(o.pan.y < 0.0, "dragging right slides the target left, got {:?}", o.pan);
        assert!(o.pan.x.abs() < 1e-4, "and stays in the view plane, got {:?}", o.pan);

        // Cursor DOWN (screen Y grows downward): the target rises, so the asset slides down.
        o.pan = Vec3::ZERO;
        o.pan_by_view(Vec2::new(0.0, 10.0), &persp, vh);
        assert!(o.pan.z > 0.0, "dragging down raises the target, got {:?}", o.pan);

        // A pan TRANSLATES the camera rigidly — eye and target move together. If only the target
        // moved this would silently become an orbit.
        let after = o.camera(radius);
        let d_eye = after.position - before.position;
        let d_tgt = after.target - before.target;
        assert!((d_eye - d_tgt).length() < 1e-4, "eye and target must move together");
        let dir_before = (before.position - before.target).normalize();
        let dir_after = (after.position - after.target).normalize();
        assert!((dir_before - dir_after).length() < 1e-5, "a pan must not rotate the view");

        // 1:1 at the look-at depth: dragging a full viewport height moves exactly the height
        // visible there, so the pan neither crawls on a large asset nor bolts on a small one.
        o.pan = Vec3::ZERO;
        o.pan_by_view(Vec2::new(0.0, vh), &persp, vh);
        let visible_h = 2.0 * o.dist(radius) * (FOV_Y * 0.5).tan();
        assert!(
            (o.pan.length() - visible_h).abs() < 1e-2,
            "expected a 1:1 pan of {visible_h}, got {}",
            o.pan.length()
        );

        // Scale follows the framing: the same drag on a 10× larger asset pans 10× further.
        let mut big = Orbit { yaw: 0.0, pitch: 0.0, dist_scale: 2.4, zoom: 1.0, pan: Vec3::ZERO };
        let big_cam = big.camera(radius * 10.0);
        big.pan_by_view(Vec2::new(0.0, vh), &big_cam, vh);
        assert!((big.pan.length() / o.pan.length() - 10.0).abs() < 1e-3, "pan scales with framing");

        // An ORTHOGRAPHIC quad pans in ITS OWN plane, not the perspective one — that is the whole
        // reason `pan_by_view` takes a camera. TOP looks down +Z with +Y up the panel (the Z-up
        // `EDITOR_QUADS`), so its screen-right is +X.
        let top = Camera {
            position: Vec3::new(0.0, 0.0, 400.0),
            target: Vec3::ZERO,
            up: Vec3::Y,
            fov_y_radians: FOV_Y,
            near: 0.01,
            far: 1200.0,
            ortho_height: Some(220.0),
        };
        let mut t = Orbit { yaw: 0.0, pitch: 0.0, dist_scale: 2.4, zoom: 1.0, pan: Vec3::ZERO };
        t.pan_by_view(Vec2::new(10.0, 0.0), &top, vh);
        assert!(t.pan.x < 0.0, "dragging right in TOP slides the target along -X, got {:?}", t.pan);
        assert!(t.pan.z.abs() < 1e-4, "and never along the view's OWN axis, got {:?}", t.pan);
        // An ortho camera states its visible height, so the scale is that height over the viewport
        // — using the perspective formula here would pan at the wrong rate entirely.
        t.pan = Vec3::ZERO;
        t.pan_by_view(Vec2::new(0.0, vh), &top, vh);
        assert!((t.pan.length() - 220.0).abs() < 1e-2, "1:1 against the ortho height");
    }

    /// The wheel zooms MULTIPLICATIVELY and moves every panel together — a perspective view that
    /// zoomed while the ortho panels stayed put would make the four disagree about scale.
    #[test]
    fn wheel_zoom_is_proportional_and_moves_all_four_views() {
        let mut o = Orbit::default();
        let radius = 100.0_f32;
        let (d0, r0) = (o.dist(radius), o.ortho_radius(radius));

        o.zoom_by(1.0); // one notch in
        assert!(o.dist(radius) < d0, "scrolling up moves the eye closer");
        assert!(o.ortho_radius(radius) < r0, "and tightens the ortho framing with it");
        let (fd, fr) = (o.dist(radius) / d0, o.ortho_radius(radius) / r0);
        assert!((fd - fr).abs() < 1e-5, "perspective and ortho must zoom by the SAME factor");

        o.zoom_by(-1.0); // and back out
        assert!((o.dist(radius) - d0).abs() / d0 < 0.02, "a notch out ~undoes a notch in");

        // Clamped at both ends: the wheel can never invert the camera or run away to nothing.
        for _ in 0..500 {
            o.zoom_by(1.0);
        }
        assert!(o.zoom >= 0.05 && o.dist(radius) > 0.0, "zoom floors, never inverts");
        for _ in 0..500 {
            o.zoom_by(-1.0);
        }
        assert!(o.zoom <= 6.0, "and ceilings");

        // A dead or non-finite wheel event must not drift the framing.
        let z = o.zoom;
        o.zoom_by(0.0);
        o.zoom_by(f32::NAN);
        assert_eq!(o.zoom, z, "a no-op wheel event changes nothing");
    }

    /// The fitting body is the REFERENCE a piece is placed against, so its MESH must load, not
    /// just its bones — judging whether hair sits on the skull needs a shape, not a stick figure.
    /// Real content; skips without it.
    #[test]
    fn the_fitting_body_loads_its_mesh_for_the_reference_view() {
        let Some(base) = BasePreview::load() else {
            eprintln!("skipping: no content tree");
            return;
        };
        assert!(!base.globals.is_empty(), "the fitting body has a skeleton");
        assert!(
            !base.verts.is_empty(),
            "the fitting body must carry a MESH — `fitting_base` prefers the ~3.3k-tri \
             GolemBase_Low, which is far under the budget"
        );
        assert!(base.verts.len() <= BASE_MESH_BUDGET, "and it fits the upload budget");
        // A well-formed triangle list that indexes only real vertices — a bad one would fault the
        // draw rather than merely look wrong.
        assert!(!base.indices.is_empty() && base.indices.len() % 3 == 0, "a triangle list");
        let n = base.verts.len() as u32;
        assert!(base.indices.iter().all(|i| *i < n), "every index is inside the vertex list");

        // Framing now comes from the MESH when there is one, so the stage floor is the SOLE of the
        // foot rather than the lowest JOINT (the ankle sits well above the sole — `ANKLE_FRACTION`).
        // Getting this wrong floats the body above its own grid.
        assert!(base.floor < 0.0, "the recentred floor is below the origin");
        let lowest_vert = base.verts.iter().map(|v| v.position[2]).fold(f32::MAX, f32::min);
        let lowest_joint = base.globals.iter().map(|g| g.w_axis.z).fold(f32::MAX, f32::min);
        assert!(
            lowest_vert <= lowest_joint + 1e-3,
            "the mesh must reach at or below the lowest joint (sole {lowest_vert}, joint {lowest_joint})"
        );
    }

    // ── the canonical-pattern gates (rule E5AFBBAB) ────────────────────────────────
    // Three assertions that hold Clayworks on the Quartermaster shape. They exist
    // because the regression they guard is INVISIBLE: a hand-composed surface looks
    // identical on screen to a configured one, so only a test can tell them apart.

    /// COMPOSITION IS DATA. The bench is a proto the scene CONFIGURES, and the
    /// per-scene HUD script that used to hand-assemble this surface out of
    /// `Cell`/`Row`/`Stack` builders is gone — asserted by absence, so the legacy
    /// idiom cannot come back quietly the way it did in Sablework (81885AB7).
    #[test]
    fn the_bench_is_a_data_proto_the_scene_only_configures() {
        let reg = builtin_templates();
        for proto in
            ["clayworks_bench", "clayworks_row", "clayworks_bank", "clayworks_offset", "clayworks_import_card"]
        {
            assert!(reg.contains_key(proto), "`{proto}` is registered in ui_templates.json");
        }

        // No unexpanded `template` node survives `build_tree` — the expand seam is
        // INSIDE it, so the scene and every gate above walk the same tree.
        fn unexpanded(n: &UiNode, out: &mut Vec<String>) {
            if let Some(t) = n.template.as_ref() {
                out.push(t.clone());
            }
            n.children.iter().for_each(|c| unexpanded(c, out));
        }
        let tree = AssetPipeline::new().build_tree(Vec2::new(1600.0, 900.0));
        let mut left = Vec::new();
        unexpanded(&tree, &mut left);
        assert!(left.is_empty(), "these protos never resolved: {left:?}");

        let lua = std::path::Path::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../content/sensorium/scripts/hud_assetpipeline.lua"
        ));
        assert!(
            !lua.exists(),
            "hud_assetpipeline.lua is back — composition belongs in ui_templates.json, not a per-scene script"
        );
    }

    /// THE SCREEN DECLARES ITS INPUT AS DATA (S9). The root carries `on_<signal>`
    /// props and `UiIntents::of` collects every one of them — a suffix that names no
    /// `ActionSignal` is warned-and-skipped, which would silently produce a dead
    /// binding, so the count is the assertion.
    #[test]
    fn the_screen_declares_its_input_as_data() {
        let tree = AssetPipeline::new().build_tree(Vec2::new(1600.0, 900.0));
        let declared: Vec<&String> =
            tree.props.keys().filter(|k| k.starts_with("on_")).collect();
        assert_eq!(declared.len(), 7, "seven declared intents, got {declared:?}");

        let intents = UiIntents::of(&tree);
        use flicker_input_core::ActionSignal;
        for (signal, name) in [
            (ActionSignal::Menu, "pause_open"),
            (ActionSignal::Confirm, "wf_next"),
            (ActionSignal::Cancel, "wf_back"),
            (ActionSignal::TabNext, "wf_next"),
            (ActionSignal::TabPrev, "wf_back"),
            (ActionSignal::ModeNext, "gizmo_next"),
            (ActionSignal::ModePrev, "gizmo_prev"),
        ] {
            assert_eq!(
                intents.result_for(signal),
                Some(name),
                "{signal:?} resolves — an uncollected intent is a pad press bound to nothing"
            );
        }
    }

    /// EVERY DECLARED INTENT REACHES THE DISPATCHER. A result name the scene never
    /// acts on is dead hardware: the button is bound, the signal fires, nothing
    /// happens. `pause_open` is read in `update`'s pause arm; the rest are results
    /// the frame's dispatch consumes.
    #[test]
    fn every_declared_intent_reaches_the_dispatcher() {
        load_shipped_strings();
        let mut ed = AssetPipeline::new();

        // The gizmo arms — the pad's L2/R2 reach into the mode toggle. Cycles both ways.
        assert_eq!(ed.gizmo_mode, GizmoMode::Translate);
        for expect in [GizmoMode::Rotate, GizmoMode::Scale, GizmoMode::Translate] {
            ed.apply_gizmo_results(&fired("gizmo_next"));
            assert_eq!(ed.gizmo_mode, expect, "gizmo_next cycles forward and wraps");
        }
        for expect in [GizmoMode::Scale, GizmoMode::Rotate, GizmoMode::Translate] {
            ed.apply_gizmo_results(&fired("gizmo_prev"));
            assert_eq!(ed.gizmo_mode, expect, "gizmo_prev cycles back and wraps");
        }

        // `wf_next` / `wf_back` are the workflow runtime's own vocabulary — the
        // footer buttons and A/B/bumpers all land on these two names.
        park(&mut ed, "conform");
        ed.apply_workflow_results(&fired("wf_back"));
        assert_eq!(ed.wf.step(), "task", "wf_back — the declared Cancel/TabPrev result — steps the rail");
    }

    /// The rail SKIPS what a class does not use — BY DEFINITION now: the prop workflow
    /// simply has no `attach` step (Attach defines the sockets a BODY OFFERS; a prop mounts
    /// TO one, authored at Conform), so neither Next nor Back can ever stop there, and its
    /// rail is Workflow · Mount · Review. No content needed: this is the dispatch itself.
    #[test]
    fn a_non_character_workflow_has_no_attach_step() {
        load_shipped_strings();
        let mut ed = AssetPipeline::new();
        ed.dispatch_workflow(Some(AssetClass::Prop));
        assert_eq!(ed.wf_def.steps.len(), 3, "task → conform → review");
        assert!(
            ed.wf_def.steps.iter().all(|s| s.id != "attach"),
            "the prop definition carries no character-only Attach"
        );

        // Back from Review lands on Mount — there is no attach between to strand the user on.
        park(&mut ed, "review");
        ed.apply_workflow_results(&fired("wf_back"));
        assert_eq!(ed.wf.step(), "conform", "Back hops straight to the rig (Mount) view");

        // The rail: three chips, the conform chip named for its definition's role.
        let m = ed.hud_model();
        assert_eq!(
            m.text("wf_conform_title"),
            Some("Mount"),
            "the prop definition names its conform stop"
        );
        assert!(!m.is_on("wf_attach_show"), "no character-only Attach chip");
        assert!(m.is_on("wf_task_show") && m.is_on("wf_conform_show") && m.is_on("wf_review_show"));
        assert_eq!(m.number("wf_step_n"), Some(3.0), "the footer counts three stops");
    }

    /// The rail chips ride the workflow RUNTIME's published binds: the current step lights
    /// `workflow.chip.active`, walked-past steps read `visited`, later ones `todo`, and
    /// every chip title arrives PRE-LOCALIZED through the stringtable — while the step's
    /// surface key gates its content subtree, exclusively.
    #[test]
    fn rail_chips_ride_the_workflow_runtime_binds() {
        load_shipped_strings();
        let mut ed = AssetPipeline::new();
        park(&mut ed, "conform"); // the rig-edit view, reached right after choosing a workflow
        let m = ed.hud_model();
        // Load / Analyze / Classify are gone (the workflow selector + inline parse/conform
        // replaced them), so the character rail reads Workflow · Rig · Attach · Review.
        assert_eq!(m.text("wf_task_title"), Some("Workflow"));
        assert_eq!(m.text("wf_conform_title"), Some("Rig"), "the character definition's conform stop");
        assert_eq!(m.text("wf_attach_title"), Some("Attach"));
        assert_eq!(m.text("wf_review_title"), Some("Review"));
        // The current step lights via the runtime's style path; behind reads visited, ahead todo.
        assert_eq!(m.text("wf_conform_style"), Some("workflow.chip.active"));
        assert_eq!(m.text("wf_task_style"), Some("workflow.chip.visited"));
        assert_eq!(m.text("wf_attach_style"), Some("workflow.chip.todo"));
        // All four chips of the character definition are shown, and the footer counts them.
        for id in ["task", "conform", "attach", "review"] {
            assert!(m.is_on(&format!("wf_{id}_show")), "{id} chip shown");
        }
        assert_eq!(m.number("wf_step_n"), Some(4.0));
        // The step SURFACES (namespaced `wf_step_<id>`) gate the content subtrees,
        // exclusively on the current step.
        assert!(m.is_on("wf_step_conform"), "the current step's surface is on");
        assert!(
            !m.is_on("wf_step_task") && !m.is_on("wf_step_attach") && !m.is_on("wf_step_review"),
            "the rest are off"
        );
    }

    /// Next never advances past a step whose declared needs are missing — on the Workflow
    /// page with no source open the `conform` gate refuses — and the LAST step has no next:
    /// its forward button is the bench's own Restart, which stays live.
    #[test]
    fn the_gate_refuses_without_a_source() {
        let mut ed = AssetPipeline::new();
        assert!(!ed.wf.can_next(&ed.wf_doc()), "no source open — `conform` needs one");
        ed.apply_workflow_results(&fired("wf_next"));
        assert_eq!(ed.wf.step(), "task", "the refused advance holds the page");
        park(&mut ed, "review");
        assert!(!ed.wf.can_next(&ed.wf_doc()), "Review is the last step");
        assert!(ed.hud_model().is_on("next_enabled"), "…whose button stays live as Restart");
    }

    /// "Restart" on the last page is SCENE policy over the runtime: it drops the finished
    /// asset and rebuilds the DEFAULT (character) workflow back on Task — and the embedded
    /// `ui_workflows.json` genuinely ships both dispatch targets.
    #[test]
    fn restart_on_review_returns_to_task_with_the_default_definition() {
        let mut ed = AssetPipeline::new();
        assert!(ed.workflows.contains_key(WF_CHARACTER) && ed.workflows.contains_key(WF_PROP));
        ed.dispatch_workflow(Some(AssetClass::Prop));
        ed.pending_class = Some(AssetClass::Prop);
        park(&mut ed, "review");
        ed.apply_workflow_results(&fired("wf_next"));
        assert_eq!(ed.wf.step(), "task", "the wizard is back on the entry page");
        assert!(ed.source.is_none() && ed.pending_class.is_none(), "the per-asset state dropped");
        assert_eq!(ed.wf_def.steps.len(), 4, "…and the default (character) definition is restored");
    }

    /// A conform edit arms the Back DISCARD GUARD: Back holds the page and raises the
    /// `wf_discard` surface, "keep editing" stays put, "discard" steps back — and the
    /// dialog genuinely draws through the real tree (the `choice_dialog` instance the HUD
    /// gates on that surface), so the guard is not an invisible state.
    #[test]
    fn dirty_back_raises_the_discard_dialog() {
        load_shipped_strings();
        let mut ed = AssetPipeline::new();
        park(&mut ed, "conform");
        ed.wf.set_dirty(true); // as a slider/gizmo edit would through `update`
        ed.apply_workflow_results(&fired("wf_back"));
        assert_eq!(ed.wf.step(), "conform", "held until the dialog resolves");
        let m = ed.hud_model();
        assert!(m.is_on("wf_discard"), "the confirm dialog surface is up");

        // The dialog reaches the DRAW commands through the real Lua tree + templates.
        // The REAL tree, through the scene's own single seam — so this gate inspects
        // exactly what the app draws (8634C200: a gate that builds its own unexpanded
        // tree certifies the drift it exists to catch).
        let tree = AssetPipeline::new().build_tree(Vec2::new(1600.0, 900.0));
        let styles = load_styles(HUD_UI_ELEMENTS);
        let snap = UiInput {
            mouse: Vec2::new(-100.0, -100.0),
            clicked: false,
            down: false,
            screen: Vec2::new(1520.0, 980.0),
            typed: String::new(),
            backspace: false,
            wheel: 0.0,
        };
        let mut state = UiState::new();
        let frame = run_ui(&tree, &m, &styles, &snap, &mut state);
        let drew = frame.commands.iter().any(|c| {
            matches!(c, HudCommand::Text { text, .. } if text.contains("ARE YOU SURE?"))
        });
        assert!(drew, "the discard dialog did not draw over the bench");

        // Keep editing: the dialog closes, the step holds. Then a clean discard steps back.
        ed.apply_workflow_results(&fired("wf_discard_no"));
        assert_eq!(ed.wf.step(), "conform");
        assert!(!ed.hud_model().is_on("wf_discard"));
        ed.apply_workflow_results(&fired("wf_back"));
        ed.apply_workflow_results(&fired("wf_discard_yes"));
        assert_eq!(ed.wf.step(), "task", "discard steps back and disarms");
    }

    #[test]
    fn inspector_reports_no_source_rather_than_inventing_one() {
        load_shipped_strings(); // the empty-state line is token-resolved copy
        let ed = AssetPipeline::new();
        let lines = ed.inspector_lines();
        assert!(lines[0].contains("No source folder"), "got {lines:?}");
    }

    /// Rest frames compose parent→child, and a root bone's world frame IS its local one.
    #[test]
    fn rest_globals_compose_down_the_chain() {
        let bone = |name: &str, parent: i32, t: [f32; 3]| flicker_content::RawBone {
            name: name.into(),
            parent,
            translation: t,
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale: [1.0, 1.0, 1.0],
            inverse_bind: Mat4::IDENTITY.to_cols_array(),
        };
        let model = RawModel {
            vertices: Vec::new(),
            indices: Vec::new(),
            bones: vec![
                bone("root", -1, [0.0, 0.0, 10.0]),
                bone("child", 0, [0.0, 0.0, 5.0]),
                bone("grandchild", 1, [0.0, 0.0, 2.0]),
            ],
        };
        let (globals, parents) = rest_globals(&model, &[]);
        assert_eq!(parents, vec![-1, 0, 1]);
        assert_eq!(globals[0].w_axis.truncate(), Vec3::new(0.0, 0.0, 10.0));
        assert_eq!(globals[1].w_axis.truncate(), Vec3::new(0.0, 0.0, 15.0));
        assert_eq!(globals[2].w_axis.truncate(), Vec3::new(0.0, 0.0, 17.0));
        // The views frame about the asset's CENTRE, not the origin — in Z-up ground reckoning the
        // origin is its feet, so framing there put the body out of shot.
        let (centre, radius, floor) = model_bounds(&model, &globals);
        assert_eq!(centre, Vec3::new(0.0, 0.0, 13.5), "midway between the root and the tip");
        assert_eq!(radius, 3.5, "half the 10 → 17 span");
        // The floor is the feet plane AFTER the same `-centre` shift the viewport draws through,
        // so it is negative and lands exactly on the lowest bone — draw the stage grid at the
        // asset's soles, not at the origin (which recentring puts at its waist).
        assert_eq!(floor, -3.5, "lowest extent (z=10) recentred about 13.5");
        assert!(floor < 0.0, "a recentred floor is always below the origin");
    }
}
