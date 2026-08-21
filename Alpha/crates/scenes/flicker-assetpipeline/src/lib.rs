//! flicker-assetpipeline — **Clayworks Bench**, the asset-pipeline editor: the interactive host
//! for the `flicker-content` stages, and the producer the Quartermaster's staging tier reviews.
//!
//! The realized workflow (golden spec 4916D78B, node A3A3259C): open a folder of raw
//! sources → a step wizard classifies what is in it → matches it to the canonical
//! skeleton → bakes the one self-describing `flicker.rig` → hot-reloads in-app.
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
//! # The scene is a PAIR (five-line architecture)
//!
//! `assetpipeline.scene.json` authors the tree + this bench's style blocks AND carries
//! the three workflow DEFINITIONS in `params.workflows` (scene data lives in scene
//! files); `assetpipeline.lua` derives the presentation (rail-chip styles, bank-row
//! selection washes) from the RAW model this behaviour publishes; the Rust component
//! kinds draw. The scene owns no resolver and no bindings — the PUMP hands it resolved
//! signals, the walker consumes the screen's declared intents (`on_menu` /
//! `on_tab_next|prev` = the wizard's forward/back / `on_mode_next|prev` = the gizmo
//! cycle), and both input channels land in the ONE dispatch as result names. The
//! viewport's per-panel orbit / pan / zoom / gizmo picking stays the bespoke tier:
//! pointer edges polled inside the reserved `editor_quad` rect, plus the pump's
//! continuous `signals.axis` look while the viewport pane is ENTERED (the populous
//! world-below-walker pattern — [`EditorLayer`] consumes the camera signals then).
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

use flicker::render::{
    build_textured_verts, grid_segments_xy, Camera, Mat4, MeshDrawOptions, MeshHandle, MeshIndices,
    MeshVertex, Orbit, PbrMaps, QuadGrid, QuadView, Rect, Renderer, SceneLighting,
    SkinnedMeshHandle, SkinnedVertex, TextureHandle, TexturedMeshHandle, Vec2, Vec3,
};
use flicker::scene::{Scene, SceneInput, Transition};
use flicker::script::{HudCommand, ScriptHost, UiNode, ValueMap};
use flicker::ui::{
    render_hud, run_ui, strings, SceneDef, UiInput, UiIntents, UiState, WalkerHandler,
};
use flicker_input_core::{ActionSignal, AbstractControls, GamepadConfig, InputMap, InputState, Key};
use flicker_input_router::{Flow, InputEvent, InputHandler, RouteCtx, Router};
use flicker_shell::{PauseScene, Theme};

use flicker_content::{
    attach_world, bake_rig, bake_skin, classify_asset, conform_to_canonical, default_reference,
    fitting_base, garment_socket, parse_fbx, rename_to_canonical, reorient_to_canonical,
    scan_folder, source_maps, write_garment, write_prop, write_rig, AssetClass, AssetReport,
    ConformMode, ConformOutput, Fit, Kind, PropKind, RawModel, RenameReport, Scan, SourceMaps,
};
use flicker_mechanics::{
    autofit_capsules_from, closest_point_ray_segment, debug, drag_plane, gizmo_segments, GizmoMode,
    Shape, Volume,
};
// The clip preview's playable form: the retargeter's in-memory output decoded through the
// SAME resolve path `load_dirs` uses, then sampled per frame — no disk round-trip.
use flicker_skeletal::format::{resolve_clips, rig_bones, Bone as SkelBone, ResolvedClip, RigFile};
use flicker_skeletal::pose::{global_transforms, sample_local_poses};
use flicker_skeletal::skin;

/// ⛔ The engine-retired **Workflow runtime**, dissolved into its ONLY consumer at the
/// bench's migration (5A4528AE) — compiled BEHAVIOUR, per the security law: progression
/// logic never rides the end-user-editable Lua layer. The former `workflow.rs` file is
/// gone; this private region is its whole remaining life. Do not grow it; do not
/// re-export it.
///
/// The **Workflow** — an Orchestration with an ordinal (Aaron, ratified 2026-08-01): a
/// LINEAR sequence of step surfaces + gates + the document contract, wrapping ONE
/// `Surfaces` exclusive group. Branching is deliberately NOT here: a required branch is
/// a *different workflow definition*, chosen up front by the dispatch cards.
///
/// **The document is the pipe.** Steps never talk to each other: each reads and writes
/// the bench's one document, and declares its contract as `needs` (Model keys that must
/// be present to enter) and `yields` (keys it produces). Gating is fail-loud: `Next`
/// into a step whose needs are unmet warns and refuses — never a blank page.
///
/// **Back is destructive-guarded** (Aaron's R3): `Back` with unsaved step changes arms
/// the discard confirmation (`wf_discard` surface — the scene's `popup_panel` gates on
/// it); `wf_discard_yes` steps back and clears the dirty flag, `wf_discard_no` keeps
/// editing.
///
/// The runtime does NO IO. Workflow *definitions* are scene DATA now
/// (`assetpipeline.scene.json` `params.workflows`, loaded by [`workflows_from_params`]);
/// step documents are ephemeral scene state.
///
/// Result vocabulary (fixed, one way): `wf_next` · `wf_back` · `wf_discard_yes` ·
/// `wf_discard_no`. Published Model keys: each step's surface key (`wf_step_<id>`) ·
/// `wf_discard` · `wf_step` · `wf_step_i` / `wf_step_n` (1-based) · `wf_can_next` ·
/// per-step `wf_<id>_title` (resolved through the stringtable, so rail chips ride
/// pre-localized binds), `wf_<id>_state` (`active` / `visited` / `todo` — the pair
/// script derives the `workflow.chip.*` style path from it) and `wf_<id>_show`
/// (rail membership).
mod workflow {
    use flicker::script::ValueMap;
    use flicker::ui::{Surface, Surfaces};
    use std::collections::HashMap;

    /// One declared step of a workflow: its stable `id`, its rail title (a
    /// `$token`), the surface its subtree is gated by (defaults to the id), and the
    /// document keys it `needs` on entry and `yields` for downstream steps.
    #[derive(Clone, Debug, serde::Deserialize)]
    pub struct Step {
        pub id: String,
        /// Rail label as a stringtable `$token` (resolved at publish).
        pub title: String,
        /// The surface / `visible_bind` key of the step's subtree; `None` = the id.
        #[serde(default)]
        pub surface: Option<String>,
        /// Document keys that must be PRESENT to enter this step (fail-loud gate).
        #[serde(default)]
        pub needs: Vec<String>,
        /// Document keys this step produces — the declared forward contract.
        #[serde(default)]
        pub yields: Vec<String>,
    }

    impl Step {
        // Steps only ever DESERIALIZE from the scene file's `params.workflows` —
        // there is no hand-construction path.

        /// The Model key the step's subtree gates on: an explicit `surface`, else
        /// the NAMESPACED default `wf_step_<id>` — bare ids collided with sibling
        /// Model namespaces (and with document keys like `attach`).
        fn surface_key(&self) -> String {
            self.surface
                .clone()
                .unwrap_or_else(|| format!("wf_step_{}", self.id))
        }
    }

    /// One workflow definition as it ships in the scene file: the linear step
    /// list. (Each definition also carries a `title` per workflow — display copy
    /// the bench reads from its own tree, so the parse ignores it here.)
    #[derive(Clone, Debug, serde::Deserialize)]
    pub struct WorkflowDef {
        pub steps: Vec<Step>,
    }

    /// Load the workflow definitions from the scene def's `params.workflows` (the
    /// five-line home for scene data). Fail-LOUD on an absent or malformed block —
    /// the wizard IS this bench, so a scene file without its definitions is a
    /// content bug the error names, not a state to limp through.
    pub fn workflows_from_params(
        params: &serde_json::Map<String, serde_json::Value>,
    ) -> HashMap<String, WorkflowDef> {
        let Some(block) = params.get("workflows") else {
            tracing::error!("assetpipeline scene file carries no `params.workflows` — no wizard");
            return HashMap::new();
        };
        match serde_json::from_value(block.clone()) {
            Ok(defs) => defs,
            Err(e) => {
                tracing::error!("assetpipeline `params.workflows` failed to parse — {e}");
                HashMap::new()
            }
        }
    }

    /// The dedicated discard-confirmation surface every workflow carries.
    const DISCARD: &str = "wf_discard";

    /// A running workflow: the ordinal over one [`Surfaces`] exclusive group.
    /// Construct from a [`WorkflowDef`]'s step list, feed it the frame's results +
    /// document with [`handle`](Self::handle), and publish with
    /// [`publish`](Self::publish).
    pub struct Workflow {
        steps: Vec<Step>,
        current: usize,
        visited: Vec<bool>,
        /// The current step has unsaved changes — armed by stage logic via
        /// [`set_dirty`](Self::set_dirty); makes `Back` destructive-guarded.
        dirty: bool,
        surfaces: Surfaces,
    }

    impl Workflow {
        /// Build from the linear step list. Step surfaces form one exclusive group;
        /// the first step starts shown; the discard dialog surface rides along.
        ///
        /// Construction validates the declared contract fail-loud: a `needs` key no
        /// earlier step `yields` is warned at BUILD time (it is either scene-provided
        /// — legitimate — or unsatisfiable, and the author finds out now, not at the
        /// hundredth click of a Next that refuses).
        pub fn new(steps: Vec<Step>) -> Self {
            for (i, s) in steps.iter().enumerate() {
                for need in &s.needs {
                    let upstream = steps[..i]
                        .iter()
                        .any(|p| p.yields.iter().any(|y| y == need));
                    if !upstream {
                        tracing::warn!(
                            "workflow: step `{}` needs `{need}`, which no earlier step yields — \
                             scene-provided, or unsatisfiable",
                            s.id
                        );
                    }
                }
            }
            let mut decls: Vec<Surface> = steps
                .iter()
                .enumerate()
                .map(|(i, s)| {
                    let mut d = Surface::new(s.surface_key().as_str()).group("wf_steps");
                    if i == 0 {
                        d = d.on();
                    }
                    d
                })
                .collect();
            decls.push(Surface::new(DISCARD));
            let visited = vec![false; steps.len()];
            Self {
                steps,
                current: 0,
                visited,
                dirty: false,
                surfaces: Surfaces::new(decls),
            }
        }

        /// Build from a loaded definition.
        pub fn from_def(def: &WorkflowDef) -> Self {
            Self::new(def.steps.clone())
        }

        /// The current step's id.
        pub fn step(&self) -> &str {
            &self.steps[self.current].id
        }

        /// Mark the current step as carrying unsaved changes (stage logic calls
        /// this on edits, clears it on commit) — what arms the Back discard guard.
        pub fn set_dirty(&mut self, dirty: bool) {
            self.dirty = dirty;
        }

        /// Whether `Next` may advance: a next step exists and every document key it
        /// `needs` is present. (The last step's `Next` is the bench's own finish
        /// action — the runtime ignores it.)
        pub fn can_next(&self, doc: &ValueMap) -> bool {
            match self.steps.get(self.current + 1) {
                Some(next) => next.needs.iter().all(|k| doc.get(k).is_some()),
                None => false,
            }
        }

        /// Consume this frame's workflow results against the document: `wf_next`
        /// advances through the gate (unmet needs warn and refuse — fail loud,
        /// never a blank page); `wf_back` steps back, or arms the discard dialog
        /// when the step is dirty; `wf_discard_yes` / `wf_discard_no` resolve it.
        pub fn handle(&mut self, results: &ValueMap, doc: &ValueMap) {
            if results.is_on("wf_next") {
                match self.steps.get(self.current + 1) {
                    Some(next) => {
                        let unmet: Vec<&str> = next
                            .needs
                            .iter()
                            .filter(|k| doc.get(k).is_none())
                            .map(String::as_str)
                            .collect();
                        if unmet.is_empty() {
                            // Leaving a step that declared yields it never produced is
                            // caught HERE, at the source — not one step later when a
                            // downstream `needs` fails against it.
                            let lied: Vec<&str> = self.steps[self.current]
                                .yields
                                .iter()
                                .filter(|k| doc.get(k).is_none())
                                .map(String::as_str)
                                .collect();
                            if !lied.is_empty() {
                                tracing::warn!(
                                    "workflow: leaving `{}` without its declared yields {lied:?}",
                                    self.step()
                                );
                            }
                            self.visited[self.current] = true;
                            self.current += 1;
                            self.dirty = false;
                        } else {
                            tracing::warn!(
                                "workflow: `{}` cannot enter — needs {unmet:?} not in the document",
                                next.id
                            );
                        }
                    }
                    None => tracing::warn!(
                        "workflow: `wf_next` on the last step `{}` — finishing is the bench's own action",
                        self.step()
                    ),
                }
            }
            if results.is_on("wf_back") && self.current > 0 {
                if self.dirty {
                    self.surfaces.show(DISCARD);
                } else {
                    self.current -= 1;
                }
            }
            if results.is_on("wf_discard_yes") && self.surfaces.is_on(DISCARD) {
                self.surfaces.hide(DISCARD);
                self.dirty = false;
                self.current = self.current.saturating_sub(1);
            }
            if results.is_on("wf_discard_no") {
                self.surfaces.hide(DISCARD);
            }
        }

        /// One step's rail state — what the pair script turns into the chip's
        /// `workflow.chip.*` style path (presentation selection is the Lua's; the
        /// STATE is the runtime's).
        fn state(&self, i: usize) -> &'static str {
            if i == self.current {
                "active"
            } else if self.visited[i] {
                "visited"
            } else {
                "todo"
            }
        }

        /// Publish the workflow's whole UI state into the frame's Model: the step
        /// surfaces (exclusive on the current step), the discard dialog, the rail
        /// chips (`wf_<id>_title` pre-resolved through the stringtable so labels
        /// ride localized binds; `wf_<id>_state` as the step state the pair script
        /// styles), and the footer keys (`wf_step`, `wf_step_i`/`_n`, `wf_can_next`).
        pub fn publish(&mut self, doc: &ValueMap, m: &mut ValueMap) {
            let key = self.steps[self.current].surface_key();
            self.surfaces.set_exclusive(&key);
            self.surfaces.publish(m);
            m.set("wf_step", self.step().to_string());
            m.set("wf_step_i", (self.current + 1) as f64);
            m.set("wf_step_n", self.steps.len() as f64);
            m.set("wf_can_next", self.can_next(doc));
            for (i, s) in self.steps.iter().enumerate() {
                m.set(
                    format!("wf_{}_title", s.id),
                    flicker::ui::strings::resolve(&s.title).into_owned(),
                );
                m.set(format!("wf_{}_state", s.id), self.state(i));
                // Rail MEMBERSHIP: true for every step of the running definition, so
                // the rail derives from the data — a chip's visible_bind names this,
                // and adding a step to the definition grows the rail with no
                // hand-kept id list anywhere.
                m.set(format!("wf_{}_show", s.id), true);
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn steps() -> Vec<Step> {
            // Through the same serde path the shipped scene file's params take.
            serde_json::from_value(serde_json::json!([
                { "id": "task", "title": "$wf_step_task", "yields": ["source"] },
                { "id": "conform", "title": "$wf_step_rig", "needs": ["source"], "yields": ["rig"] },
                { "id": "review", "title": "$wf_step_review", "needs": ["rig"] }
            ]))
            .expect("test steps deserialize")
        }

        fn fired(name: &str) -> ValueMap {
            ValueMap::new().with(name, true)
        }

        #[test]
        fn linear_walk_is_gated_by_declared_needs() {
            let mut w = Workflow::new(steps());
            assert_eq!(w.step(), "task");

            // The gate refuses while `source` is absent from the document…
            let empty = ValueMap::new();
            assert!(!w.can_next(&empty));
            w.handle(&fired("wf_next"), &empty);
            assert_eq!(w.step(), "task", "unmet needs refuse the advance (warned)");

            // …and admits once the step yielded it.
            let doc = ValueMap::new().with("source", "clay/thing");
            assert!(w.can_next(&doc));
            w.handle(&fired("wf_next"), &doc);
            assert_eq!(w.step(), "conform");

            // The last step has no next: can_next is false there by definition.
            let doc = doc.with("rig", "humanoid");
            w.handle(&fired("wf_next"), &doc);
            assert_eq!(w.step(), "review");
            assert!(
                !w.can_next(&doc),
                "the last step's Next is the bench's finish"
            );
            w.handle(&fired("wf_next"), &doc);
            assert_eq!(w.step(), "review", "…and the runtime ignores it");
        }

        #[test]
        fn back_is_clean_when_not_dirty_and_guarded_when_dirty() {
            let mut w = Workflow::new(steps());
            let doc = ValueMap::new().with("source", "s").with("rig", "r");
            w.handle(&fired("wf_next"), &doc);
            assert_eq!(w.step(), "conform");

            // Clean back: straight step-back, no dialog.
            w.handle(&fired("wf_back"), &doc);
            assert_eq!(w.step(), "task");
            let mut m = ValueMap::new();
            w.publish(&doc, &mut m);
            assert!(!m.is_on("wf_discard"));

            // Dirty back: arms the discard dialog and HOLDS the step.
            w.handle(&fired("wf_next"), &doc);
            w.set_dirty(true);
            w.handle(&fired("wf_back"), &doc);
            assert_eq!(w.step(), "conform", "held until the dialog resolves");
            let mut m = ValueMap::new();
            w.publish(&doc, &mut m);
            assert!(m.is_on("wf_discard"), "the confirm dialog is up");

            // Keep editing: dialog closes, step and dirty flag stay.
            w.handle(&fired("wf_discard_no"), &doc);
            let mut m = ValueMap::new();
            w.publish(&doc, &mut m);
            assert!(!m.is_on("wf_discard"));
            assert_eq!(w.step(), "conform");

            // Discard: dialog closes, the step goes back, dirty clears.
            w.handle(&fired("wf_back"), &doc);
            w.handle(&fired("wf_discard_yes"), &doc);
            assert_eq!(w.step(), "task");
            w.handle(&fired("wf_back"), &doc);
            assert_eq!(w.step(), "task", "Back on the first step is inert");
        }

        #[test]
        fn publish_reports_rail_footer_and_exclusive_surfaces() {
            let mut w = Workflow::new(steps());
            let doc = ValueMap::new().with("source", "s");
            let mut m = ValueMap::new();
            w.publish(&doc, &mut m);

            assert!(m.is_on("wf_step_task"), "the current step's surface is on");
            assert!(
                !m.is_on("wf_step_conform") && !m.is_on("wf_step_review"),
                "the rest are off (exclusive)"
            );
            assert!(
                m.is_on("wf_task_show") && m.is_on("wf_conform_show") && m.is_on("wf_review_show"),
                "every defined step publishes rail membership"
            );
            assert_eq!(m.text("wf_step"), Some("task"));
            assert_eq!(m.number("wf_step_i"), Some(1.0));
            assert_eq!(m.number("wf_step_n"), Some(3.0));
            assert!(m.is_on("wf_can_next"));
            assert_eq!(m.text("wf_task_state"), Some("active"));
            assert_eq!(m.text("wf_conform_state"), Some("todo"));
            // (Raw-token fallback for an UNLOADED table is a strings-module property,
            // tested there — not asserted here, where sibling tests load the real table
            // process-wide.)
            assert!(m.text("wf_task_title").is_some());

            w.handle(&fired("wf_next"), &doc);
            let mut m = ValueMap::new();
            w.publish(&doc, &mut m);
            assert!(
                m.is_on("wf_step_conform") && !m.is_on("wf_step_task"),
                "the exclusive group advanced"
            );
            assert_eq!(m.text("wf_task_state"), Some("visited"));
            assert_eq!(m.text("wf_conform_state"), Some("active"));
        }

        #[test]
        fn definitions_load_from_scene_params_and_build() {
            let params: serde_json::Map<String, serde_json::Value> =
                serde_json::from_str(
                    r#"{ "workflows": {
                        "asset_import": { "title": "$wf_asset_import", "steps": [
                            { "id": "task", "title": "$wf_step_task", "yields": ["source"] },
                            { "id": "review", "title": "$wf_step_review", "needs": ["source"] }
                        ] }
                    } }"#,
                )
                .expect("test params parse");
            let defs = workflows_from_params(&params);
            let def = defs.get("asset_import").expect("definition loads");
            assert_eq!(def.steps.len(), 2);
            let mut w = Workflow::from_def(def);
            assert_eq!(w.step(), "task");
            let doc = ValueMap::new().with("source", "s");
            w.handle(&fired("wf_next"), &doc);
            assert_eq!(w.step(), "review");

            assert!(
                workflows_from_params(&serde_json::Map::new()).is_empty(),
                "an absent block errors loud and yields nothing"
            );
        }
    }
}
use workflow::{workflows_from_params, Workflow, WorkflowDef};

/// The pair script (`content/sensorium/scripts/assetpipeline.lua`) — embedded at
/// compile time like every migrated scene's; `derive()` turns the raw Model into
/// the rail-chip styles and the bank-row selection washes.
const AP_SCRIPT: &str = include_str!("../../../../content/sensorium/scripts/assetpipeline.lua");

/// The shipped scene file — the tests' copy of the authored tree + workflow
/// definitions (the runtime receives the same file through the manifest `SceneDef`).
#[cfg(test)]
const AP_SCENE: &str =
    include_str!("../../../../content/sensorium/scenes/assetpipeline.scene.json");

/// The viewport pane's id — ALSO its `tab_group` in the authored tree, which is what
/// makes it a panel for the walker's panel cursor; [`EditorLayer`] owns the camera
/// signals exactly while this pane is ENTERED (`ui_state.entered_group`).
const VIEW_PANE: &str = "ap_view";

/// Stick-look rate for the 2×2 perspective panel (rad/s at full deflection) and the
/// stick zoom rate (wheel notches/s) — the pump's `signals.axis` scaled by dt.
const STICK_LOOK_RATE: f32 = 2.4;
const STICK_ZOOM_RATE: f32 = 4.0;

/// The bespoke viewport layer BELOW the walker (the populous world pattern): while
/// the viewport pane is ENTERED the six camera signals are the editor's and stop
/// here; everywhere else they pass, and mean whatever the next handler says.
#[derive(Default)]
struct EditorLayer {
    owns_camera: bool,
}

impl InputHandler for EditorLayer {
    fn handle(&mut self, ev: &InputEvent, _rc: &mut RouteCtx) -> Flow {
        let camera_signal = matches!(
            ev.signal,
            ActionSignal::LookUp
                | ActionSignal::LookDown
                | ActionSignal::LookLeft
                | ActionSignal::LookRight
                | ActionSignal::ZoomIn
                | ActionSignal::ZoomOut
        );
        if camera_signal && self.owns_camera {
            Flow::Consumed
        } else {
            Flow::Pass
        }
    }
}

/// Skeleton overlay colours, matching the paperdoll's rig view so the two tools read alike.
/// JOINTS (the selectable balls) stay cyan; BONES (the octahedral diamonds between them) read
/// violet — distinct hues so the chain structure and the grab points never blur together in the
/// rig view (QA request, 2026-08-03).
const JOINT: [f32; 4] = [0.35, 0.9, 1.0, 1.0];
const BONE: [f32; 4] = [0.62, 0.50, 0.95, 1.0];
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

/// The wizard's spine is the dissolved **Workflow runtime** (the `workflow` module
/// above): a LINEAR step list loaded from the scene file's `params.workflows`, gated by
/// the step DOCUMENT the scene publishes each frame ([`AssetPipeline::wf_doc`]). These
/// are the definitions the Task page's cards dispatch between — branching lives BETWEEN
/// definitions, never inside one, so the old in-spine "skip Attach for a non-character"
/// conditional is now simply a definition without the step.
///
/// `task` is the ENTRY page of ALL THREE — a workflow-selection card grid (Import
/// Character / Accessory / Prop / Animation). The user DECLARES the workflow there rather
/// than the tool guessing it; choosing a card opens the folder dialog, and ingest → parse
/// → classify → conform all run inline (see `open`), so the asset lands DIRECTLY on the
/// rig-edit view. The old Load / Analyze / Classify stops are gone: each was a page whose
/// only action was "click Next", so the work now happens automatically and the user is
/// not made to walk through a stage that asks nothing of them.
const WF_CHARACTER: &str = "import_character";
const WF_PROP: &str = "import_prop";
const WF_ANIMATION: &str = "import_animation";

/// The Clip page's side-by-side pair — ROOT MOTION travels its authored path, IN PLACE
/// treadmills at the origin, both sampled at the same shared tick so the comparison is
/// honest (the ruled Clip-role UX: pick one, the other, or both). Corner labels ride
/// the same composite path as [`EDITOR_QUADS`]' PERSP/TOP/… tags.
const CLIP_VIEWS: [QuadView; 2] = [
    QuadView {
        label: "ROOT MOTION",
        label_flipped: "ROOT MOTION",
        ortho: None,
    },
    QuadView {
        label: "IN PLACE",
        label_flipped: "IN PLACE",
        ortho: None,
    },
];

/// The bake-preview page's single full-rect view: the baked body playing the shared idle.
const BAKE_VIEWS: [QuadView; 1] = [QuadView {
    label: "BAKE · IDLE",
    label_flipped: "BAKE · IDLE",
    ortho: None,
}];

/// The shared idle the bake preview plays, under the package root — the same clip the
/// Controller Tester's pack opens on, so the smoke test judges against the real thing.
const BAKE_PREVIEW_CLIP: &str = "retarget/clips/locomotion/In-Place/idle_neutral.json";

/// What the Conform stage IS for the loaded asset.
///
/// Conform is ONE step in the rail carrying SEVERAL ROLES, because "rig this" means something
/// different per class: a character maps its skeleton onto the canonical bone set, a prop/garment
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
    /// Animation: the active BVH retargeted onto the reference skeleton, previewed as the
    /// side-by-side RootMotion / In-Place pair; the user picks what Commit keeps.
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
            Self::Clip => "$ap_preview_both_variants_pick_what_commit_k",
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
    /// First visible row — the bone list is the full canon in a 150px box, so it pages.
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
        Self {
            socket: 0,
            offset: [0.0; 3],
            rot: [0.0; 3],
            scale: [1.0; 3],
            uniform: 1.0,
        }
    }
}

impl PropFit {
    fn socket_name(&self) -> &'static str {
        SOCKETS
            .get(self.socket)
            .map(|(id, _)| *id)
            .unwrap_or("pelvis")
    }
}

/// The loaded source folder — what Load produced, plus what each later stage added.
/// The Animation workflow's WORKING STATE — the active BVH retargeted onto the reference
/// skeleton IN MEMORY, both variants resolved and playable, plus the exact emitted JSON so
/// Commit writes precisely what the preview showed. Built by `prepare_clip` (idempotent,
/// the sibling of `analyze`/`conform`); a pick clears it to re-run.
struct ClipPreview {
    /// The reference skeleton the clips were baked onto (decoded from the emitted clip).
    bones: Vec<SkelBone>,
    /// Per-bone parent indices, in `bones` order — the overlay helpers' shape.
    parents: Vec<i32>,
    ip: ResolvedClip,
    rm: ResolvedClip,
    /// Ticks — both variants share it (same source frames, same 60 Hz canon).
    duration: u32,
    /// Rest-pose framing: half-extent, ground height, and centre.
    radius: f32,
    floor: f32,
    ip_center: Vec3,
    /// RootMotion framing widened to the pelvis's planar TRAVEL, so the walk stays in shot.
    rm_center: Vec3,
    rm_radius: f32,
    /// The retargeter's verbatim output — what Commit writes (no re-run, no drift).
    variants: flicker_content::retarget::ClipVariants,
}

impl ClipPreview {
    /// Decode the retargeter's in-memory output into a playable preview: parse both clip
    /// JSONs, take the embedded reference skeleton, resolve the tracks through the SAME
    /// path `load_dirs` uses, and derive the two panels' framing.
    fn resolve(variants: flicker_content::retarget::ClipVariants) -> Result<Self, String> {
        let ip_file: RigFile = serde_json::from_value(variants.in_place.clone())
            .map_err(|e| format!("in-place clip: {e}"))?;
        let rm_file: RigFile = serde_json::from_value(variants.root_motion.clone())
            .map_err(|e| format!("root-motion clip: {e}"))?;
        let bones = rig_bones(&ip_file);
        if bones.is_empty() {
            return Err("clip carries no skeleton".into());
        }
        let parents: Vec<i32> = bones.iter().map(|b| b.parent).collect();
        let ip = resolve_clips(&ip_file, &bones, false)
            .pop()
            .ok_or("in-place clip resolved empty")?;
        let rm = resolve_clips(&rm_file, &bones, false)
            .pop()
            .ok_or("root-motion clip resolved empty")?;
        let duration = ip.duration_ticks.max(rm.duration_ticks).max(1);

        // Rest framing from the skeleton's own joint extent — a clip has no mesh.
        let rest_locals: Vec<Mat4> = bones.iter().map(|b| b.local).collect();
        let rest = global_transforms(&bones, &rest_locals);
        let mut min = Vec3::splat(f32::MAX);
        let mut max = Vec3::splat(f32::MIN);
        for g in &rest {
            let p = g.w_axis.truncate();
            min = min.min(p);
            max = max.max(p);
        }
        let radius = ((max - min).length() * 0.5).max(1.0);
        let ip_center = (min + max) * 0.5;

        // The RootMotion panel frames the TRAVEL: the pelvis carries the planar
        // translation (In-Place pins exactly that), so its key extent widens the shot.
        let (mut tmin, mut tmax) = (Vec2::ZERO, Vec2::ZERO);
        if let Some(pi) = bones.iter().position(|b| b.name == "pelvis") {
            if let Some(tr) = rm.tracks.iter().find(|t| t.bone == pi) {
                for k in &tr.keys {
                    let p = Vec2::new(k.translation[0], k.translation[1]);
                    tmin = tmin.min(p);
                    tmax = tmax.max(p);
                }
            }
        }
        let travel = (tmax + tmin) * 0.5;
        Ok(Self {
            rm_center: ip_center + Vec3::new(travel.x, travel.y, 0.0),
            rm_radius: radius + (tmax - tmin).length() * 0.5,
            bones,
            parents,
            ip,
            rm,
            duration,
            radius,
            floor: min.z,
            ip_center,
            variants,
        })
    }
}

/// The Rig stage's SMOKE TEST (Aaron 2026-08-20): the page right after joint editing plays
/// the SHARED idle on the body EXACTLY as Commit writes it — same bake path
/// ([`AssetPipeline::character_bake_model`]), same GPU skinning the runtime uses — so a bad
/// placement is caught in-bench instead of after promote. Dropped on leaving the page and
/// rebuilt on every entry, so it always reflects the latest joint work.
struct BakePreview {
    bones: Vec<SkelBone>,
    /// Parent index per bone — the skeleton overlay's topology, cached once at build.
    parents: Vec<i32>,
    clip: ResolvedClip,
    mesh: SkinnedMeshHandle,
    bone_count: u32,
    /// Rest framing: centre/half-extent/ground of the baked joints, source space (Z-up cm).
    centre: Vec3,
    radius: f32,
    floor: f32,
}

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
    /// Where a re-opened rig came from ("staging" / "package"), `None` on the vendor-FBX
    /// path — the inspector surfaces it so a staged reload is never silent about its source
    /// (Aaron 2026-08-20: the promoted fit lives in PACKAGE after Quartermaster's move-only
    /// promote empties staging, and a silent fallthrough read as lost work).
    reopened: Option<&'static str>,
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
    /// The Animation workflow's retargeted, playable preview — `None` until
    /// `prepare_clip` runs (and for every other class, always).
    clip: Option<ClipPreview>,
}

impl Source {
    /// The asset name the pipeline would bake under — the source folder's own name.
    fn asset_name(&self) -> &str {
        self.dir
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("asset")
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
        let Some(parsed) = self.parsed.as_ref() else {
            return;
        };
        for p in &mut self.attach {
            p.bone = parsed.bone_index(p.parent);
        }
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
    /// Task-page toggle (Aaron 2026-08-20): before parsing the chosen folder's FBX, look for
    /// this asset's already-STAGED rig (`staging/characters/<name>/<name>.json`) and re-open
    /// THAT instead — the re-process loop, so an already-fitted body can be adjusted further
    /// and re-committed without redoing the joint work from the vendor source.
    prefer_staged: bool,
    /// Import DIAGNOSTIC (Task page): stage this vendor rig EXACTLY as provided — skip the joint
    /// derivation and the reorient in [`conform_to_canonical`] (`ConformMode::AsProvided`), keeping
    /// Meshy's own skeleton and skin, and only completing the bone set. Off by default; the standard
    /// canonical conform runs unless a human opts into this to test the raw rig against the clips.
    as_provided: bool,
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
    /// The Clip page's side-by-side pair, tiled in the SAME holder rect as the 2×2 —
    /// exactly one of the two grids renders per frame (the frame graph's offscreen
    /// passes reset the draw queues, so they must never both run).
    clip_grid: Option<QuadGrid>,
    /// One camera per clip panel (`CLIP_VIEWS` order: ROOT MOTION, IN PLACE).
    clip_orbits: [Orbit; 2],
    /// The shared playback clock, in CLIP TICKS (fractional between samples); both
    /// panels read it so the variants stay in lockstep. Wraps at the clip duration.
    clip_tick: f32,
    /// The Preview page's single view (same exclusivity rule as `clip_grid`: exactly one
    /// grid renders per frame).
    bake_grid: Option<QuadGrid>,
    bake_orbit: Orbit,
    /// The bake preview's playback clock, in idle ticks (fractional between samples).
    bake_tick: f32,
    /// The Preview page's baked body — built on entry, dropped on leaving the page.
    bake: Option<BakePreview>,
    /// The side-by-side pick — what Commit keeps (ruled: one, the other, or both).
    variant_ip: bool,
    variant_rm: bool,
    show_skeleton: bool,
    /// Cached last cursor, for orbit dragging.
    last_mouse: Vec2,
    // ── Pause plumbing, as the shell expects (built in `enter`, handed to PauseScene). ──
    /// Mouse-look tuning handed to the pause overlay. The PUMP owns the live action
    /// maps (input-P3) — the scene resolves nothing itself.
    controls: AbstractControls,
    gamepad_config: GamepadConfig,
    ui_theme: Option<Theme>,
    // ── HUD (the scene pair) ──
    /// The AUTHORED tree off the manifest's def (the five-line split), walked every
    /// frame. `take`n around the walk so the walker can borrow it beside the mutable
    /// UI state.
    authored: Option<UiNode>,
    /// The pair script (`assetpipeline.lua`) — derives the rail-chip styles and the
    /// bank-row selection washes from the raw Model each frame. `None` only if it
    /// failed to load; the chips and banks then draw their base styles.
    script: Option<ScriptHost>,
    /// The bespoke viewport layer BELOW the walker in the dispatch chain — owns the
    /// six camera signals exactly while the viewport pane is ENTERED.
    editor: EditorLayer,
    /// The screen's declarative signal bindings (S9), collected from the authored
    /// root ONCE (`on_menu = "pause_open"`, the wizard's `on_tab_*`, the gizmo's
    /// `on_mode_*`). The walker layer consumes a declared signal and the ONE
    /// dispatch below maps the fired name onto its scene action.
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
    Textured {
        mesh: TexturedMeshHandle,
        albedo: TextureHandle,
        maps: PbrMaps,
    },
    Flat(MeshHandle),
}

impl Uploaded {
    /// Draw at `world`. `flat_tint` applies ONLY to the untextured path (see the type docs).
    fn draw(self, r: &mut Renderer, world: Mat4, flat_tint: [f32; 4]) {
        match self {
            Uploaded::Textured { mesh, albedo, maps } => {
                r.draw_textured_mesh_pbr(mesh, albedo, maps, world, MeshDrawOptions::default())
            }
            Uploaded::Flat(h) => r.draw_mesh(
                h,
                world,
                MeshDrawOptions {
                    tint: flat_tint,
                    ..Default::default()
                },
            ),
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

    let flat: Vec<usize> = idx
        .iter()
        .map(|&i| i as usize)
        .filter(|&i| i < verts.len())
        .collect();
    let tv = build_textured_verts(
        0..flat.len(),
        |k| verts[flat[k]].position,
        |k| verts[flat[k]].normal,
        |k| uvs[flat[k]],
    );
    let li: Vec<u32> = (0..tv.len() as u32).collect();
    let mesh = r.upload_textured_mesh(&tv, MeshIndices::U32(&li));
    let normal = maps
        .normal
        .as_deref()
        .and_then(|p| load_map(r, cache, p, false));
    let roughness = maps
        .roughness
        .as_deref()
        .and_then(|p| load_map(r, cache, p, false));
    let metalness = maps
        .metalness
        .as_deref()
        .and_then(|p| load_map(r, cache, p, false));
    Uploaded::Textured {
        mesh,
        albedo,
        // `emit: None` — the ingest classifier (`flicker_content::SourceMaps`) has no
        // emissive slot yet, so nothing here has a map to bind even though
        // `flicker_skeletal::format::Material` already carries an `emit` field and
        // the content standard mandates the map. TRACKED GAP, not an omission:
        // closing it means teaching `source_maps` the `_Emit` suffix.
        maps: PbrMaps {
            normal,
            roughness,
            metalness,
            ao: None,
            emit: None,
        },
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
        let globals: Vec<Mat4> = ibind
            .iter()
            .map(|m| Mat4::from_cols_array(m).inverse())
            .collect();

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
                .map(|x| MeshVertex {
                    position: x.p,
                    normal: x.n,
                    material: 0,
                })
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
        let dir = base_path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."));
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
        Some(Self {
            names,
            parents,
            globals,
            ibind,
            centre,
            radius,
            floor,
            verts,
            uvs,
            indices,
            maps,
        })
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
        Self::shipped()
    }
}

impl AssetPipeline {
    /// The runtime constructor — the manifest hands in the authored `SceneDef`
    /// (the five-line split): the tree + this bench's style blocks + the workflow
    /// definitions all come from `assetpipeline.scene.json`.
    pub fn new(def: &SceneDef) -> Self {
        Self::from_parts(def.tree.clone(), def.styles.clone(), &def.params)
    }

    /// A bench on the SHIPPED scene file — the seam a test drives without an
    /// app, exercising the same authored tree + definitions the runtime gets.
    #[cfg(test)]
    pub fn shipped() -> Self {
        let def = SceneDef::parse("assetpipeline", AP_SCENE)
            .expect("the shipped assetpipeline.scene.json parses");
        Self::from_parts(def.tree, def.styles, &def.params)
    }

    #[cfg(not(test))]
    pub fn shipped() -> Self {
        // Outside tests the manifest is the only construction path; a def-less
        // bench would be a blank screen, so `Default` routes here loudly.
        unreachable!("AssetPipeline is built from the manifest's SceneDef")
    }

    fn from_parts(
        authored: Option<UiNode>,
        scene_styles_json: Option<serde_json::Value>,
        params: &serde_json::Map<String, serde_json::Value>,
    ) -> Self {
        if authored.is_none() {
            tracing::error!("assetpipeline: the scene def declares no `tree` — no UI will draw");
        }
        let ui_styles = flicker::ui::load_shared_styles(scene_styles_json.as_ref());
        // The screen's declared bindings (S9), read off the authored root ONCE.
        let ui_intents = authored.as_ref().map(UiIntents::of).unwrap_or_default();
        let script = match ScriptHost::new(AP_SCRIPT, "assetpipeline.lua") {
            Ok(h) => Some(h),
            Err(e) => {
                tracing::error!("assetpipeline.lua failed to load — base styles only: {e}");
                None
            }
        };
        // The workflow spine. The definitions are SCENE DATA (`params.workflows`), so
        // a missing block is a content bug the manifest gate catches — expect() over
        // limping on with no wizard. The character definition is also the pre-dispatch
        // default: `ConformRole::of(None)` already treats an unclassified asset as a
        // character, so the empty bench shows the same four-stop rail it always has.
        let workflows = workflows_from_params(params);
        let wf_def = workflows
            .get(WF_CHARACTER)
            .expect("assetpipeline.scene.json ships the import_character definition")
            .clone();
        let wf = Workflow::from_def(&wf_def);
        Self {
            workflows,
            wf_def,
            wf,
            pending_class: None,
            pending_prop: None,
            prefer_staged: false,
            as_provided: false,
            source: None,
            grid: None,
            quad_rect: None,
            orbits: [Orbit::default(); 4],
            clip_grid: None,
            clip_orbits: [Orbit::default(); 2],
            clip_tick: 0.0,
            bake_grid: None,
            bake_orbit: Orbit::default(),
            bake_tick: 0.0,
            bake: None,
            variant_ip: true,
            variant_rm: true,
            show_skeleton: true,
            last_mouse: Vec2::ZERO,
            controls: AbstractControls::default(),
            gamepad_config: GamepadConfig::default(),
            ui_theme: None,
            authored,
            script,
            editor: EditorLayer::default(),
            ui_intents,
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
        let Some(dir) = rfd::FileDialog::new()
            .set_title("Open asset source folder")
            .pick_folder()
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
                // pre-selected so the wizard is never stuck. The ANIMATION workflow's candidates
                // are the folder's BVH clips instead — same picker, different kind.
                let (candidates, error): (Vec<PathBuf>, Option<String>) = if self.pending_class
                    == Some(AssetClass::Animation)
                {
                    let c: Vec<PathBuf> = scan.of_kind(Kind::Bvh).map(|e| e.path.clone()).collect();
                    let e = c
                        .is_empty()
                        .then(|| format!("No BVH clips in {}", dir.display()));
                    (c, e)
                } else {
                    let c: Vec<PathBuf> = scan.candidates().map(|e| e.path.clone()).collect();
                    let e = c
                        .is_empty()
                        .then(|| format!("No riggable mesh in {}", dir.display()));
                    (c, e)
                };
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
                    reopened: None,
                    attach: Self::new_attach(),
                    attach_sel: 0,
                    fit: PropFit::default(),
                    fit_window: 0,
                    committed: None,
                    error,
                    clip: None,
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
                    // The Task page's staged-reload preference: adopt the asset's already-staged
                    // rig when one exists, and `analyze`/`conform` below become no-ops (both
                    // early-return once their outputs are present). Absence falls through to the
                    // vendor-FBX path unchanged.
                    if self.prefer_staged {
                        self.adopt_staged();
                    }
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

    /// Re-open the asset's already-PROCESSED rig instead of the vendor FBX — the Task page's
    /// "prefer staged" toggle (Aaron 2026-08-20: the re-process loop, so an already-fitted
    /// body is adjusted further instead of redoing the joint work from the source). Character
    /// path only. Searches STAGING first (in-progress work wins), then PACKAGE — because the
    /// Quartermaster's promote is MOVE-only, a promoted fit's ONE copy lives in package and
    /// staging is empty, and a staging-only search silently fell through to a fresh un-fitted
    /// conform (Aaron hit exactly this). The processed file is the bake's own output, so it
    /// loads ALREADY-CONFORMED: `rig` is pre-filled (every bone-map row Ok, zero authored
    /// offsets), which makes the `conform()` that `open` runs next a no-op — re-running the
    /// derive passes would move the human's fitted joints. Nothing processed anywhere is
    /// normal (a first import) and falls through to the FBX path.
    fn adopt_staged(&mut self) {
        let package_characters = flicker_content::roots().package().join("characters");
        for (root, origin) in [(characters_dir(), "staging"), (package_characters, "package")] {
            if self.adopt_staged_from(&root, origin) {
                return;
            }
        }
    }

    /// [`adopt_staged`] against one explicit root — so the reload path is exercisable against
    /// a scratch directory instead of the live trees, like `commit_to`. Returns whether the
    /// rig was adopted; `origin` is surfaced as the inspector's provenance line.
    fn adopt_staged_from(&mut self, root: &Path, origin: &'static str) -> bool {
        if matches!(
            self.pending_class,
            Some(AssetClass::Prop | AssetClass::Animation)
        ) {
            return false;
        }
        let Some(src) = self.source.as_mut() else {
            return false;
        };
        let name = src.asset_name().to_string();
        let path = root.join(&name).join(format!("{name}.json"));
        let mut model = match flicker_content::load_rig_raw(&path) {
            Ok(m) => m,
            Err(e) => {
                tracing::info!("no {origin} rig to re-open for {name}: {e:#}");
                return false;
            }
        };
        if model.bones.is_empty() {
            tracing::warn!("{origin} {name} carries no skeleton — falling through");
            return false;
        }
        // Chain repair on the way in: a rig staged BEFORE the splice fix carries the broken
        // chain in its bake (the golem's head hung off `neck_01`, dangling `neck_02`). The
        // splice preserves every fitted joint's world frame — only the composition is fixed —
        // so reloading is also how an existing fit is healed without redoing it.
        match flicker_content::splice_canonical_chain(&mut model, &default_reference()) {
            Ok(spliced) if !spliced.is_empty() => {
                tracing::info!("staged {name}: spliced {spliced:?} onto the canonical chain");
            }
            Ok(_) => {}
            Err(e) => tracing::warn!("staged {name}: chain check skipped: {e:#}"),
        }
        let n = model.bones.len();
        let parsed = Parsed::new(model);
        src.report = Some(classify_asset(&src.scan, Some(parsed.bones())));
        src.parsed = Some(parsed);
        src.rig = Some(Rig {
            rename: RenameReport::default(),
            out: ConformOutput::default(),
            map: vec![MapState::Ok; n],
            offsets: vec![BoneOffset::default(); n],
            sel: 0,
            focused: false,
            window: 0,
        });
        src.reopened = Some(origin);
        src.resolve_attach();
        src.error = None;
        tracing::info!("re-opened {origin} rig {name}: {n} bones");
        true
    }

    /// ANALYZE — parse the chosen FBX and measure it. Synchronous today, so a large
    /// source hitches one frame; folding the stages onto `flicker-worker::WorkerPool` is
    /// FDD Layer B and deliberately not started here.
    fn analyze(&mut self) {
        let Some(src) = self.source.as_mut() else {
            return;
        };
        // An Animation source's candidates are BVH files, not FBX meshes — its stage
        // runner is `prepare_clip`, the sibling of this one, so there is nothing to parse.
        if src.class() == Some(AssetClass::Animation) {
            return;
        }
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
                let name = src
                    .dir
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or("")
                    .to_string();
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
        // Read the import mode BEFORE borrowing `source`: the canonical path corrects the vendor
        // rig onto the reference; as-provided stages it untouched (Aaron's raw-rig diagnostic).
        let mode = if self.as_provided {
            ConformMode::AsProvided
        } else {
            ConformMode::Canonical
        };
        let Some(src) = self.source.as_mut() else {
            return;
        };
        if src.rig.is_some() {
            return;
        }
        // Conform is the CHARACTER path — it maps a biped skeleton onto the canonical
        // reference. A Prop or Animation has no such skeleton, so the wizard routes by the
        // confirmed class rather than forcing every asset through it: running it on a
        // skeleton-less mesh is exactly the misleading "no skeleton" failure. Their bake
        // paths (prop fit / clip retarget) are not wired in-app yet, and the stage says so.
        // (An unclassified asset falls through to the character path, as it always has.)
        if matches!(src.class(), Some(AssetClass::Prop | AssetClass::Animation)) {
            return;
        }
        let Some(parsed) = src.parsed.as_mut() else {
            return;
        };
        let rename = rename_to_canonical(&mut parsed.model);
        match conform_to_canonical(&mut parsed.model, &default_reference(), mode) {
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

    /// CLIP — the Animation workflow's stage runner, the sibling of `analyze`/`conform`:
    /// retarget the ACTIVE BVH onto the reference skeleton IN MEMORY and resolve both
    /// variants for playback. Nothing touches disk until Commit. Idempotent (a pick
    /// clears `clip` to re-run); a failure surfaces in the inspector, never invents.
    fn prepare_clip(&mut self) {
        let Some(src) = self.source.as_mut() else {
            return;
        };
        if src.clip.is_some()
            || src.class() != Some(AssetClass::Animation)
            || src.fbx.as_os_str().is_empty()
        {
            return;
        }
        let built = flicker_content::retarget::build_variants(&src.fbx, &default_reference())
            .map_err(|e| e.to_string())
            .and_then(ClipPreview::resolve);
        match built {
            Ok(cp) => {
                tracing::info!(
                    "retargeted {}: {} ticks, {} bones",
                    src.fbx.display(),
                    cp.duration,
                    cp.bones.len()
                );
                src.clip = Some(cp);
                src.error = None;
                self.clip_tick = 0.0;
            }
            Err(e) => src.error = Some(format!("Clip retarget failed: {e}")),
        }
    }

    /// COMMIT — bake the conformed model and write `flicker.rig` into STAGING. The authored bone
    /// offsets are baked in by re-deriving the model first, so what eventually ships is exactly
    /// what the viewport showed.
    ///
    /// This writes the bench's OUTPUT; it does not publish. The asset reaches the tree the game
    /// loads from only when the Content Manager promotes it out of staging.
    /// The character model EXACTLY as Commit bakes it: the working model cloned, the
    /// authored offsets applied, and every joint's frame translated onto the canon.
    ///
    /// THE ONE BAKE PATH — `commit_to` writes it and the Preview page plays it, so the
    /// preview can never drift from the export. The frame translation is the invariant's
    /// output gate (shared clips play absolute rotations in canonical frames): positions
    /// ship exactly as placed — Meshy's and the human's fitted joints alike — and only
    /// each bone's frame is rewritten, which is what lets the as-provided editing view
    /// stay vendor-faithful in the bench yet still produce a playable body. Idempotent on
    /// an already-canonical rig with no authored offsets; when joints WERE dragged, the
    /// limb frames re-align to the final joint layout.
    fn character_bake_model(&self) -> Result<RawModel, String> {
        let src = self.source.as_ref().ok_or("no source is open")?;
        let parsed = src.parsed.as_ref().ok_or("nothing is parsed")?;
        let mut model = parsed.model.clone();
        if let Some(rig) = src.rig.as_ref() {
            apply_offsets(&mut model, &rig.offsets);
        }
        reorient_to_canonical(&mut model, &default_reference())
            .map_err(|e| format!("Canonical frame translation failed: {e}"))?;
        Ok(model)
    }

    /// The headless half of the bake preview: bake the character exactly as Commit writes
    /// it, and resolve the SHARED idle onto the baked bones. Split from the GPU upload so
    /// tests judge the smoke test without a renderer.
    fn bake_preview_parts(&self) -> Result<(RigFile, Vec<SkelBone>, ResolvedClip), String> {
        let name = self
            .source
            .as_ref()
            .map(|s| s.asset_name().to_string())
            .ok_or("no source is open")?;
        let model = self.character_bake_model()?;
        let rig_file = bake_rig(&model, &name);
        let bones = rig_bones(&rig_file);
        if bones.is_empty() {
            return Err("the bake produced no skeleton".into());
        }
        let idle = flicker_content::roots().package().join(BAKE_PREVIEW_CLIP);
        let text = flicker_content::package::read_text(&idle)
            .map_err(|e| format!("shared idle {}: {e}", idle.display()))?;
        let file: RigFile =
            serde_json::from_str(&text).map_err(|e| format!("shared idle: {e}"))?;
        let clip = resolve_clips(&file, &bones, false)
            .pop()
            .ok_or("the shared idle resolved empty")?;
        if clip.tracks.is_empty() {
            return Err("the shared idle resolved onto NO bones — names diverged from canon".into());
        }
        Ok((rig_file, bones, clip))
    }

    /// Build the Preview page's playable body if it is not already built: bake, frame,
    /// upload. A failure surfaces in the inspector like any stage failure — never silent.
    fn ensure_bake_preview(&mut self, renderer: &mut Renderer) {
        if self.bake.is_some() {
            return;
        }
        match self.bake_preview_parts() {
            Ok((rig_file, bones, clip)) => {
                // Rest framing from the baked joints (source space, Z-up cm), exactly like
                // the clip preview: centre for the orbit target, floor for the lattice.
                let rest: Vec<Mat4> = bones.iter().map(|b| b.local).collect();
                let globals = global_transforms(&bones, &rest);
                let mut min = Vec3::splat(f32::MAX);
                let mut max = Vec3::splat(f32::MIN);
                for g in &globals {
                    let p = g.w_axis.truncate();
                    min = min.min(p);
                    max = max.max(p);
                }
                let centre = (min + max) * 0.5;
                let radius = ((max - min).length() * 0.5).max(1.0);
                let floor = min.z;
                let verts: Vec<SkinnedVertex> = rig_file
                    .mesh
                    .vertices
                    .iter()
                    .map(|v| SkinnedVertex {
                        position: v.p,
                        normal: v.n,
                        uv: v.uv,
                        joints: v.joints,
                        weights: v.weights,
                    })
                    .collect();
                let indices: Vec<u32> = if rig_file.mesh.indices.is_empty() {
                    (0..verts.len() as u32).collect()
                } else {
                    rig_file.mesh.indices.clone()
                };
                let mesh = renderer.upload_skinned_mesh(&verts, MeshIndices::U32(&indices));
                let bone_count = bones.len() as u32;
                tracing::info!(
                    bones = bones.len(),
                    verts = verts.len(),
                    clip = %clip.name,
                    "bake preview built"
                );
                self.bake_tick = 0.0;
                let parents: Vec<i32> = bones.iter().map(|b| b.parent).collect();
                self.bake = Some(BakePreview {
                    bones,
                    parents,
                    clip,
                    mesh,
                    bone_count,
                    centre,
                    radius,
                    floor,
                });
            }
            Err(e) => {
                if let Some(s) = self.source.as_mut() {
                    if s.error.as_deref() != Some(e.as_str()) {
                        tracing::warn!("bake preview: {e}");
                        s.error = Some(e);
                    }
                }
            }
        }
    }

    /// THE PREVIEW PAGE: one full-rect view of the baked body playing the shared idle,
    /// GPU-skinned through the same palette path the runtime uses — the Rig stage's smoke
    /// test. Judge it, then go Back to adjust joints or Next toward Export.
    fn render_bake_preview(&mut self, renderer: &mut Renderer, base_layer: f32) {
        self.ensure_bake_preview(renderer);
        // The grid is built in `enter` and confined to the HUD's holder rect every frame
        // in `update`, exactly like `clip_grid` — never created here, where it would run
        // a frame unconfined and composite over the whole HUD.
        let (Some(bp), Some(grid)) = (self.bake.as_ref(), self.bake_grid.as_ref()) else {
            return;
        };
        let tick = (self.bake_tick as u32).min(bp.clip.duration_ticks.saturating_sub(1));
        let locals = sample_local_poses(&bp.bones, &bp.clip, tick, true);
        let globals = global_transforms(&bp.bones, &locals);
        let palette = skin::palette(&bp.bones, &globals);
        let recentre = Mat4::from_translation(-bp.centre);
        let ground = grid_segments_xy(bp.radius * 0.25, bp.radius * 2.5, bp.floor - bp.centre.z);
        // The Display panel's Skeleton toggle, honoured here exactly as in the rig view: violet
        // diamond bones + cyan joint balls on the POSED frames, recentred with the mesh and drawn
        // as an overlay so the lines read through the flesh — where the bones are versus where
        // the mesh went.
        let (bones, balls) = if self.show_skeleton {
            let min_r = (bp.radius * BALL_MIN_FRAC).max(0.2);
            let max_r = (bp.radius * BALL_MAX_FRAC).max(min_r);
            let radii =
                debug::joint_ball_radii(&bp.parents, &globals, BALL_LEN_FRAC, min_r, max_r);
            let bones = debug::bone_diamonds(recentre, &bp.parents, &globals, BONE_WAIST_FRAC);
            let mut balls = Segments::new();
            for (i, g) in globals.iter().enumerate() {
                let c = recentre.transform_point3(g.w_axis.truncate());
                balls.extend(debug::wireframe(&Shape::Sphere {
                    center: c,
                    radius: radii[i],
                }));
            }
            (bones, balls)
        } else {
            (Segments::new(), Segments::new())
        };
        let cameras = [grid.camera(
            0,
            self.bake_orbit.ortho_radius(bp.radius),
            &self.bake_orbit.camera(bp.radius),
        )];
        let (mesh, bone_count) = (bp.mesh, bp.bone_count);
        grid.render_with(renderer, base_layer + 2.0, &cameras, |r, _view| {
            r.set_scene(SceneLighting::default());
            if !ground.is_empty() {
                r.draw_lines(&ground, GROUND);
            }
            r.draw_skinned_instanced(mesh, &[recentre], &palette, bone_count);
            if !bones.is_empty() {
                r.draw_lines_overlay(&bones, BONE);
            }
            if !balls.is_empty() {
                r.draw_lines_overlay(&balls, JOINT);
            }
        });
    }

    fn commit(&mut self) {
        let root = self.commit_root();
        self.commit_to(&root);
    }

    /// Where THIS source's commit lands, by what it is: clip variants → the shared
    /// retarget library; ENVIRONMENT props → their own `staging/props/` tier (a tree is
    /// not a character); characters, garments and worn accessories → `staging/characters/`.
    /// Every root is STAGING — the Quartermaster's promote pass is the only door into
    /// `package/`, the one tree the engine loads content from.
    fn commit_root(&self) -> PathBuf {
        let class = self.source.as_ref().and_then(|s| s.class());
        let prop = self.source.as_ref().map(|s| s.prop);
        match (class, prop) {
            (Some(AssetClass::Animation), _) => clips_dir(),
            (Some(AssetClass::Prop), Some(PropKind::Environment)) => props_dir(),
            _ => characters_dir(),
        }
    }

    /// The commit itself, against an explicit root — so the write path is exercisable against a
    /// scratch directory instead of the engine's live content tree. Dispatches by CLASS: a Skin
    /// bakes the conformed character (offsets applied), a Prop bakes a static mesh, a clothing Prop
    /// bakes a garment SKINNED onto the base body. `flicker-content` owns every bake; this only
    /// routes and records the outcome.
    fn commit_to(&mut self, root: &Path) {
        // The ANIMATION path first: it has no parsed FBX — its input is the retargeted
        // preview, its output the PICKED clip variants (ruled: one, the other, or both).
        if self.source.as_ref().and_then(|s| s.class()) == Some(AssetClass::Animation) {
            self.commit_clip_to(root);
            return;
        }
        // Export must never be a SILENT no-op (QA 2026-08-03: "doesn't always end up
        // producing an object in the staging folder" — this early-return was why): with
        // nothing parsed there is nothing to bake, and the refusal lands where every
        // other stage failure does, in the inspector.
        {
            let Some(src) = self.source.as_mut() else {
                return;
            };
            if src.parsed.is_none() {
                src.error = Some(
                    "Nothing is parsed — the source never loaded, so there is nothing to commit."
                        .to_string(),
                );
                return;
            }
        }
        // Read everything under a shared borrow, then drop it before the write + the mutable
        // outcome record (so the borrow checker stays happy across the class dispatch).
        let (class, prop, name, model_result, has_rig, fit, fbx, mounts) = {
            let Some(src) = self.source.as_ref() else {
                return;
            };
            let Some(parsed) = src.parsed.as_ref() else {
                return;
            };
            // A CHARACTER bakes through the ONE shared path (`character_bake_model`) — the
            // same model the Preview page plays, so the preview can never drift from the
            // export. A prop/garment ships the parse as-is (no offsets, no frame gate).
            let model_result = if matches!(src.class(), Some(AssetClass::Skin) | None)
                && src.rig.is_some()
            {
                self.character_bake_model()
            } else {
                Ok(parsed.model.clone())
            };
            // The human-authored placement the Attach stage tuned — what Commit bakes in.
            let fit = Fit {
                socket: src.fit.socket_name().to_string(),
                offset: src.fit.offset,
                rot_deg: src.fit.rot,
                scale: src.fit.scale,
                uniform: src.fit.uniform,
            };
            // The character's authored attach POINTS (the Attach stage's output) — handed
            // to the bake so the six tuned placements SHIP in the rig's `attach_points`
            // block instead of being discarded at export (the audited third-step gap).
            let mounts: Vec<flicker_content::MountPoint> = src
                .attach
                .iter()
                .map(|p| flicker_content::MountPoint {
                    id: p.id.to_string(),
                    bone: p.parent.to_string(),
                    offset: p.offset,
                })
                .collect();
            // The mesh file this came from — the prop/garment bakes read its FOLDER for the vendor's
            // texture maps, and its NAME tells one set piece's maps from another's.
            (
                src.class(),
                src.prop,
                src.asset_name().to_string(),
                model_result,
                src.rig.is_some(),
                fit,
                src.fbx.clone(),
                mounts,
            )
        };
        let model = match model_result {
            Ok(m) => m,
            Err(e) => {
                if let Some(s) = self.source.as_mut() {
                    s.error = Some(e);
                }
                return;
            }
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
                // Animation never reaches this dispatch — routed to `commit_clip_to` above.
                Some(AssetClass::Animation) => unreachable!("animation commits via commit_clip_to"),
                // Character: requires the conform to have produced a rig.
                _ => {
                    if has_rig {
                        write_rig(&model, &fbx, &name, &out, &mounts).map_err(|e| e.to_string())
                    } else {
                        Err("Conform has not run — nothing to commit.".to_string())
                    }
                }
            });

        let Some(src) = self.source.as_mut() else {
            return;
        };
        match result {
            Ok(()) => {
                tracing::info!("committed {}", out.display());
                src.committed = Some(out);
                src.error = None;
            }
            Err(e) => src.error = Some(e),
        }
    }

    /// The Animation commit: write the PICKED variants of the previewed clip under
    /// `<root>/<Set>/{In-Place,RootMotion}/<stem>.json` — the retargeter's VERBATIM
    /// output, so what lands in staging is exactly what the side-by-side showed.
    /// Root-parameterized for scratch-dir tests, like the character path.
    fn commit_clip_to(&mut self, root: &Path) {
        let (ip, rm) = (self.variant_ip, self.variant_rm);
        let outcome = {
            let Some(src) = self.source.as_ref() else {
                return;
            };
            match src.clip.as_ref() {
                None => Err("Clip retarget has not run — nothing to commit.".to_string()),
                Some(_) if !ip && !rm => {
                    Err("Pick at least one variant (Root Motion / In-Place) to commit.".to_string())
                }
                Some(cp) => flicker_content::retarget::write_variants(
                    &cp.variants,
                    &root.join(src.asset_name()),
                    ip,
                    rm,
                )
                .map_err(|e| e.to_string()),
            }
        };
        let Some(src) = self.source.as_mut() else {
            return;
        };
        match outcome {
            Ok(paths) => {
                tracing::info!(
                    "committed {} clip variant(s) for {}",
                    paths.len(),
                    src.asset_name()
                );
                src.committed = paths.into_iter().next();
                src.error = None;
            }
            Err(e) => src.error = Some(e),
        }
    }

    /// The Clip page's draw: both variants sampled at the SHARED tick, each into its
    /// own panel — ROOT MOTION through a frame widened to its planar travel, IN PLACE
    /// at rest framing. Same conventions as the 2×2 (recentre to origin, violet bones
    /// under cyan joint balls, depth-tested ground lattice).
    fn render_clip(&mut self, renderer: &mut Renderer, base_layer: f32) {
        let Some(grid) = self.clip_grid.as_ref() else {
            return;
        };
        let Some(cp) = self.source.as_ref().and_then(|s| s.clip.as_ref()) else {
            return;
        };
        let tick = (self.clip_tick as u32).min(cp.duration.saturating_sub(1));
        let pose = |clip: &ResolvedClip| {
            let locals = sample_local_poses(&cp.bones, clip, tick, false);
            global_transforms(&cp.bones, &locals)
        };
        let min_r = (cp.radius * BALL_MIN_FRAC).max(0.2);
        let max_r = (cp.radius * BALL_MAX_FRAC).max(min_r);
        // One panel's overlay set: diamonds + balls recentred on its framing, plus the
        // ground lattice at the skeleton's feet (sized to the panel's own extent).
        let panel = |globals: &[Mat4], centre: Vec3, extent: f32| {
            let recentre = Mat4::from_translation(-centre);
            let radii = debug::joint_ball_radii(&cp.parents, globals, BALL_LEN_FRAC, min_r, max_r);
            let bones = debug::bone_diamonds(recentre, &cp.parents, globals, BONE_WAIST_FRAC);
            let mut balls = Segments::new();
            for (i, g) in globals.iter().enumerate() {
                let c = recentre.transform_point3(g.w_axis.truncate());
                balls.extend(debug::wireframe(&Shape::Sphere {
                    center: c,
                    radius: radii[i],
                }));
            }
            let ground = grid_segments_xy(extent * 0.25, extent * 2.5, cp.floor - centre.z);
            (bones, balls, ground)
        };
        let panels = [
            panel(&pose(&cp.rm), cp.rm_center, cp.rm_radius),
            panel(&pose(&cp.ip), cp.ip_center, cp.radius),
        ];
        let cameras: Vec<Camera> = [cp.rm_radius, cp.radius]
            .into_iter()
            .enumerate()
            .map(|(i, r)| {
                grid.camera(
                    i,
                    self.clip_orbits[i].ortho_radius(r),
                    &self.clip_orbits[i].camera(r),
                )
            })
            .collect();
        grid.render_with(renderer, base_layer + 2.0, &cameras, |r, view| {
            r.set_scene(SceneLighting::default());
            let (bones, balls, ground) = &panels[view];
            if !ground.is_empty() {
                r.draw_lines(ground, GROUND);
            }
            if !bones.is_empty() {
                r.draw_lines_overlay(bones, BONE);
            }
            if !balls.is_empty() {
                r.draw_lines_overlay(balls, JOINT);
            }
        });
    }

    /// The engine-requirement checks the Review stage reports — each computed from real state, so
    /// a red line is a real blocker and not a placeholder.
    fn requirements(&self) -> Vec<(bool, String)> {
        let Some(src) = self.source.as_ref() else {
            return Vec::new();
        };
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
                        format!(
                            "{} ({verts} {})",
                            r("$ap_req_garment_mesh_present"),
                            r("$ap_verts")
                        ),
                    ),
                    (true, r("$ap_req_skins_onto_the_canonical_base")),
                ];
            }
            Some(AssetClass::Prop) => {
                return vec![
                    (
                        verts > 0,
                        format!(
                            "{} ({verts} {})",
                            r("$ap_req_prop_mesh_present"),
                            r("$ap_verts")
                        ),
                    ),
                    (true, r("$ap_req_socket_fit_authored_in_the_paperdoll")),
                ];
            }
            Some(AssetClass::Animation) => {
                return vec![
                    (
                        src.clip.is_some(),
                        r("$ap_req_clip_retargets_onto_the_reference"),
                    ),
                    (
                        self.variant_ip || self.variant_rm,
                        r("$ap_req_at_least_one_variant_selected"),
                    ),
                ];
            }
            _ => {}
        }
        // Character (Skin / unclassified): reported in BAKED terms — the +1 is the root `bake_rig`
        // synthesizes — so the figure here is the one the shipped rig will carry.
        let conformed = src.parsed.as_ref().map(|p| p.bones()).unwrap_or(0);
        let baked = if conformed == 0 { 0 } else { conformed + 1 };
        let mut out = vec![(
            conformed == CONFORMED_BONES,
            format!(
                "{} ({baked} / {REFERENCE_BONES} {})",
                r("$ap_req_skeleton_conforms"),
                r("$ap_bones")
            ),
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
                        format!(
                            "{} {}",
                            rig.rename.unmapped.len(),
                            r("$ap_req_source_bones_unmapped")
                        )
                    },
                ));
            }
        }
        let resolved = (0..src.attach.len())
            .filter(|i| self.attach_resolved(*i))
            .count();
        out.push((
            resolved == src.attach.len(),
            format!(
                "{} ({resolved} / {})",
                r("$ap_req_attach_points_on_valid_parents"),
                src.attach.len()
            ),
        ));
        out.push((
            src.textures > 0,
            format!(
                "{} ({} {})",
                r("$ap_req_textures_masks_resolved"),
                src.textures,
                r("$ap_found")
            ),
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
            let has_mesh = src
                .parsed
                .as_ref()
                .map(|p| !p.model.vertices.is_empty())
                .unwrap_or(false);
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
                    .map(|v| MeshVertex {
                        position: v.p,
                        normal: v.n,
                        material: 0,
                    })
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
        let cam = grid.camera(
            cell,
            o.ortho_radius(self.view_radius),
            &o.camera(self.view_radius),
        );
        cam.pick_ray(local, vp).map(|ray| (cell, ray))
    }

    /// The gizmo mode as the radio group's bound NAME — the radio is the one
    /// NAME-keyed picker (its contract: a row's literal string id, echoed as text),
    /// and a mode is a named choice, not an index into an ordered collection. ONE
    /// representation: these ids are the authored radio `value`s, nothing else.
    fn gizmo_mode_id(&self) -> &'static str {
        match self.gizmo_mode {
            GizmoMode::Translate => "translate",
            GizmoMode::Rotate => "rotate",
            GizmoMode::Scale => "scale",
        }
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
        let origin = recentre.transform_point3(
            globals
                .get(sel)
                .map(|g| g.w_axis.truncate())
                .unwrap_or(Vec3::ZERO),
        );
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
                    self.gizmo_drag = Some((
                        DragMode::Deform {
                            normal: d.normalize_or_zero(),
                            restore,
                        },
                        (o, d),
                    ));
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
            self.gizmo_drag = Some((
                DragMode::Reposition {
                    normal: d.normalize_or_zero(),
                },
                (o, d),
            ));
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
        let Some(src) = self.source.as_mut() else {
            return;
        };
        let Some(slot) = src.rig.as_mut().and_then(|r| r.offsets.get_mut(sel)) else {
            return;
        };
        slot.t[0] += dt.x;
        slot.t[1] += dt.y;
        slot.t[2] += dt.z;
        let offsets = src
            .rig
            .as_ref()
            .map(|r| r.offsets.clone())
            .unwrap_or_default();
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
        let Some(src) = self.source.as_mut() else {
            return;
        };
        if let Some(slot) = src.rig.as_mut().and_then(|r| r.offsets.get_mut(sel)) {
            *slot = offset;
        }
        let offsets = src
            .rig
            .as_ref()
            .map(|r| r.offsets.clone())
            .unwrap_or_default();
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
            (
                src.fit,
                matches!(src.class(), Some(AssetClass::Skin) | None),
            )
        };
        if is_char {
            return None;
        }
        let mesh = self.ensure_source_mesh(r)?;
        if self.base.is_none() {
            self.base = BasePreview::load();
        }

        let base = self.base.as_ref()?;
        let socket_name = SOCKETS
            .get(fit.socket)
            .map(|(id, _)| *id)
            .unwrap_or("pelvis");
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
        let base_joints =
            debug::bone_diamonds(recentre, &base.parents, &base.globals, BONE_WAIST_FRAC);
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

    /// The gizmo-mode dispatcher — POINTER and PAD reaching one piece of state by
    /// two routes. The REAL radio trio binds `gizmo_mode` by mode NAME (the radio is
    /// the name-keyed picker; the walker echoes the model value when untouched, so
    /// the read is idempotent);
    /// `gizmo_next` / `gizmo_prev` are the names the screen's `on_mode_next` /
    /// `on_mode_prev` declarations fire, and they CYCLE (with wrap) so one
    /// controller axis covers all three modes without a button per mode.
    ///
    /// Split out of `update` so the declared-intent gate can drive it directly: an
    /// intent whose arm lives inline in the frame loop can only be tested by running a
    /// frame, and a gate that cannot run is a gate that stops being written.
    fn apply_gizmo_results(&mut self, results: &ValueMap) {
        if let Some(id) = results.text("gizmo_mode") {
            self.gizmo_mode = match id {
                "rotate" => GizmoMode::Rotate,
                "scale" => GizmoMode::Scale,
                _ => GizmoMode::Translate,
            };
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
            "preview" => ("$ap_preview_title", "$ap_preview_hint"),
            "attach" => (
                "$ap_attach_points",
                "$ap_position_hold_holster_and_belt_attach_po",
            ),
            "review" => (
                "$ap_review_export",
                "$ap_verify_engine_requirements_then_export",
            ),
            // "task" — and the unreachable arm a bad definition would land in.
            _ => (
                "$ap_select_workflow",
                "$ap_choose_the_kind_of_asset_you_are_importi",
            ),
        };
        m.set("step_title", strings::resolve(title).into_owned());
        m.set("step_hint", strings::resolve(hint).into_owned());
        m.set("show_skeleton", self.show_skeleton);
        m.set("show_base", self.show_base);
        m.set("show_collision", self.show_collision);
        m.set("mirror", self.mirror_joints);
        // The Task page's staged-reload preference — checkbox state, mirrored like the toggles.
        m.set("prefer_staged", self.prefer_staged);
        // The Task page's "import as provided" diagnostic — checkbox state, same as prefer_staged.
        m.set("as_provided", self.as_provided);
        // The Preview page's readout — real, measured facts from the built bake (empty until built).
        m.set(
            "preview_status",
            match self.bake.as_ref() {
                Some(bp) => format!(
                    "{} · tick {}/{} · {} bones",
                    bp.clip.name,
                    (self.bake_tick as u32).min(bp.clip.duration_ticks.saturating_sub(1)),
                    bp.clip.duration_ticks,
                    bp.bones.len()
                ),
                None => String::new(),
            },
        );
        // The Clip page's variant pick — checkbox state, mirrored like the toggles above.
        m.set("variant_rm", self.variant_rm);
        m.set("variant_ip", self.variant_ip);

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
                m.set(
                    "asset_name",
                    strings::resolve("$ap_no_asset_loaded").into_owned(),
                );
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
            m.set(
                format!("insp_{i}"),
                lines.get(i).cloned().unwrap_or_default(),
            );
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
            strings::resolve(if step == "review" {
                "$wf_restart"
            } else {
                "$wf_next"
            })
            .into_owned(),
        );
        // The piece picker rides on the rig stage, and only when the folder holds a choice of meshes
        // (a weapon set, an outfit) — a single-mesh folder never shows it.
        let several = self
            .source
            .as_ref()
            .map(|s| s.candidates.len() > 1)
            .unwrap_or(false);
        m.set("on_pick", step == "conform" && several);
        // The picker's header names what it lists — meshes for the rig path, BVH
        // clips for the animation path (the rows themselves are class-agnostic).
        m.set(
            "pick_head",
            strings::resolve(if role == ConformRole::Clip {
                "$ap_clips_in_this_folder"
            } else {
                "$ap_meshes_in_this_folder"
            })
            .into_owned(),
        );
        self.pick_model(&mut m);

        // Per-step content gates by the step's SURFACE key ("task" / "attach" / "review"),
        // published by the runtime below — exactly one subtree is live. CONFORM is the one
        // step carrying SEVERAL ROLES, so its panels refine the step with the role (scene
        // logic — the runtime knows steps, not classes): a Skin gets the bone map and its
        // offsets; a prop/garment gets the mount socket + placement (its rig IS that
        // binding); an animation gets an honest "not wired" instead of controls
        // addressing nothing.
        let at_conform = step == "conform";
        m.set(
            "on_conform_skeleton",
            at_conform && role == ConformRole::Skeleton,
        );
        m.set("on_conform_mount", at_conform && role == ConformRole::Mount);
        m.set("on_conform_clip", at_conform && role == ConformRole::Clip);
        self.conform_model(&mut m);
        self.attach_model(&mut m);
        self.fit_model(&mut m);
        self.review_model(&mut m);
        // The workflow runtime's OWN publish: the step surfaces (exclusive on the current
        // step — the "task"/"attach"/"review" gates above), the `wf_discard` dialog, the
        // rail chips (`wf_<id>_title` / `wf_<id>_state` / `wf_<id>_show`) and the footer
        // step keys (`wf_step`, `wf_step_i`/`_n`, `wf_can_next`).
        self.wf.publish(&doc, &mut m);
        m
    }

    /// The frame's full model: the raw variables plus the pair script's derived
    /// presentation values (rail-chip styles, bank-row washes) folded over them,
    /// and the transient `sig_<name>` mirror (S9) riding the same ONE publish.
    fn model(&mut self) -> ValueMap {
        let raw = self.hud_model();
        let mut m = raw.clone();
        if let Some(script) = &self.script {
            if let Err(e) = script.set_model(&raw) {
                tracing::error!("assetpipeline: publishing the model to the script failed: {e}");
            }
            match script.derive() {
                Ok(Some(derived)) => m.extend(derived),
                Ok(None) => {}
                Err(e) => tracing::error!("assetpipeline: derive() failed: {e}"),
            }
        }
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
            }
            m.set("bone_sel", 0.0);
            m.set("bone_window", 0.0);
            m.set(
                "rig_headline",
                strings::resolve("$ap_conform_has_not_run").into_owned(),
            );
            m.set("rig_legend", "");
            m.set(
                "rig_sel",
                strings::resolve("$ap_no_bone_selected").into_owned(),
            );
            m.set("rig_progress", 0.0);
            for (k, _) in OFFSET_AXES {
                m.set(k, 0.0);
            }
            m.set("gizmo_mode", self.gizmo_mode_id());
            m.set("has_rig", false);
            return;
        };
        m.set("has_rig", true);
        let (ok, review, auto) = rig.counts();
        let total = rig.map.len();
        m.set(
            "rig_headline",
            format!(
                "{}      {ok} / {total}",
                strings::resolve("$ap_source_internal_rig")
            ),
        );
        m.set(
            "rig_legend",
            format!(
                "\u{25cf} {review} {}      \u{25cf} {auto} {}",
                strings::resolve("$ap_need_review"),
                strings::resolve("$ap_auto_inferred")
            ),
        );
        m.set(
            "rig_progress",
            if total > 0 {
                ok as f64 / total as f64
            } else {
                0.0
            },
        );

        // The visible window of the bone map — six rows of the full canon list. The
        // selection is the slot WASH (the pair script derives it from the two
        // cursors below), so the row label carries no selection glyph.
        for i in 0..BONE_ROWS {
            let idx = rig.window + i;
            match (parsed.model.bones.get(idx), rig.map.get(idx)) {
                (Some(b), Some(state)) => {
                    let edited = rig.offsets.get(idx).is_some_and(|o| !o.is_zero());
                    m.set(
                        format!("bone_{i}"),
                        format!(
                            "{:<22}{}{}",
                            b.name,
                            strings::resolve(state.tag()),
                            if edited { " *" } else { "" }
                        ),
                    );
                    m.set(format!("bone_{i}_color"), state.color());
                }
                _ => {
                    m.set(format!("bone_{i}"), "");
                    m.set(format!("bone_{i}_color"), MapState::Ok.color());
                }
            }
        }
        // The bank cursors, as NUMBERS (1B64FF03) — the pair script turns them
        // into the six rows' selection washes.
        m.set("bone_sel", rig.sel as f64);
        m.set("bone_window", rig.window as f64);
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

        let name = parsed
            .model
            .bones
            .get(rig.sel)
            .map(|b| b.name.as_str())
            .unwrap_or("—");
        m.set(
            "rig_sel",
            format!("{name} \u{2192} {}", strings::resolve("$ap_offset")),
        );
        let o = rig.offsets.get(rig.sel).copied().unwrap_or_default();
        for (i, (key, _)) in OFFSET_AXES.into_iter().enumerate() {
            m.set(key, if i < 3 { o.t[i] as f64 } else { o.roll as f64 });
        }
        m.set("gizmo_mode", self.gizmo_mode_id());
    }

    /// ATTACH bindings: the six points, their parent bones, and the selected point's
    /// offsets. The selection is the slot WASH (derived from `att_sel_idx`), so the
    /// row labels carry no selection glyph.
    fn attach_model(&self, m: &mut ValueMap) {
        let Some(src) = self.source.as_ref() else {
            for i in 0..ATTACH_POINTS.len() {
                m.set(format!("att_{i}"), "");
            }
            m.set("att_sel_idx", 0.0);
            m.set(
                "att_sel",
                strings::resolve("$ap_no_point_selected").into_owned(),
            );
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
                    "{:<18}{} {}{}",
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
        }
        m.set("att_sel_idx", src.attach_sel as f64);
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
                    let name = p.file_name().and_then(|s| s.to_str()).unwrap_or("");
                    m.set(format!("pick_{i}"), name);
                }
                None => {
                    m.set(format!("pick_{i}"), "");
                }
            }
        }
        // The bank cursors, as NUMBERS — the pair script derives the row washes.
        m.set("pick_sel", sel as f64);
        m.set("pick_window", window as f64);
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
                    m.set(format!("sock_{i}"), strings::resolve(label).into_owned());
                }
                None => {
                    m.set(format!("sock_{i}"), "");
                }
            }
        }
        // The bank cursors, as NUMBERS — the pair script derives the row washes.
        m.set("sock_sel", fit.socket as f64);
        m.set("sock_window", window as f64);
        let last = (window + SOCKET_ROWS).min(SOCKETS.len());
        m.set(
            "sock_page",
            format!(
                "{}\u{2013}{} {} {}",
                window + 1,
                last,
                strings::resolve("$ap_of"),
                SOCKETS.len()
            ),
        );
        m.set("sock_prev_enabled", window > 0);
        m.set("sock_next_enabled", window + SOCKET_ROWS < SOCKETS.len());
        // "Mount:" reuses the step's own token — one copy source for the noun.
        m.set(
            "fit_socket",
            format!(
                "{}: {}",
                strings::resolve("$wf_step_mount"),
                strings::resolve(
                    SOCKETS
                        .get(fit.socket)
                        .map(|(_, l)| *l)
                        .unwrap_or("\u{2014}")
                )
            ),
        );
        let vals = [
            fit.offset[0],
            fit.offset[1],
            fit.offset[2],
            fit.rot[0],
            fit.rot[1],
            fit.rot[2],
        ];
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
                    m.set(
                        format!("req_{i}"),
                        format!("{}  {text}", if *ok { "\u{2713}" } else { "\u{2717}" }),
                    );
                    m.set(
                        format!("req_{i}_color"),
                        if *ok {
                            "assetpipeline.map.ok"
                        } else {
                            "assetpipeline.map.fail"
                        },
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
            strings::resolve(if committed.is_some() {
                "$ap_exported"
            } else {
                "$ap_export_to_staging"
            })
            .into_owned(),
        );
        m.set("commit_enabled", reqs.iter().all(|(ok, _)| *ok));
        // Once a piece is baked, offer the loop straight back to the folder's picker — a weapon set
        // or an outfit is imported one piece at a time, and walking Back five stages to reach the
        // list again is not a workflow.
        let more = self
            .source
            .as_ref()
            .map(|s| s.candidates.len() > 1)
            .unwrap_or(false);
        m.set("has_committed", committed.is_some() && more);
    }

    fn inspector_title(&self) -> String {
        let token = match self.wf.step() {
            // Conform dispatches by role, so the inspector title tracks the rail: a prop reads
            // "Mount", an animation "Clips" — never the character "Rig" label over a page that is neither.
            "conform" => self.conform_role().title(),
            "preview" => "$ap_preview_title",
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
            return vec![
                r("$ap_no_source_folder_open"),
                r("$ap_load_an_asset_folder_to_begin"),
            ];
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
                match cls {
                    // The Clip page reports the RETARGETED preview's real facts — file,
                    // length, and the variant pick Commit will honour.
                    Some(AssetClass::Animation) => match src.clip.as_ref() {
                        None => out.push(r("$ap_retarget_runs_when_this_stage_is_reached")),
                        Some(cp) => {
                            let secs = cp.duration as f32 / cp.ip.tick_rate_hz.max(1) as f32;
                            out.push(format!("{:<11}{}", r("$ap_clip"), src.file_name()));
                            out.push(format!(
                                "{:<11}{} · {:.1}s",
                                r("$ap_duration"),
                                cp.duration,
                                secs
                            ));
                            let mark = |on: bool| if on { "[x]" } else { "[ ]" };
                            out.push(format!(
                                "{:<11}{} {} · {} {}",
                                r("$ap_variants"),
                                mark(self.variant_rm),
                                r("$ap_root_motion"),
                                mark(self.variant_ip),
                                r("$ap_in_place"),
                            ));
                        }
                    },
                    _ => {
                        out.push(r("$ap_rigging_maps_a_biped_skeleton_onto_the"));
                        out.push(r("$ap_67_bone_reference_this_asset_has_none"));
                        out.push(r("$ap_so_it_is_skipped"));
                        out.push(String::new());
                        if is_clothing {
                            out.push(r("$ap_export_skins_this_garment_onto_the_base"));
                            out.push(r("$ap_nearest_vertex_weight_transfer_fit_baked"));
                        } else {
                            out.push(r("$ap_export_bakes_the_static_prop_mesh_its_so"));
                            out.push(r("$ap_and_fit_are_authored_in_the_paperdoll"));
                        }
                    }
                }
            }
            "conform" => match src.rig.as_ref() {
                None => out.push(r("$ap_conform_runs_when_this_stage_is_reached")),
                Some(rig) => {
                    // A re-opened rig says so FIRST — the joints on screen are the last
                    // committed fit, not a fresh conform, and that must never be ambiguous.
                    if let Some(origin) = src.reopened {
                        out.push(match origin {
                            "package" => r("$ap_reopened_from_package"),
                            _ => r("$ap_reopened_from_staging"),
                        });
                        out.push(String::new());
                    }
                    // `{:<10}` / `{:<15}` reproduce the original aligned columns around
                    // the resolved words (en-us widths; other locales just realign).
                    out.push(format!("{:<10}{}", r("$ap_renamed"), rig.rename.renamed));
                    out.push(format!("{:<10}{}", r("$ap_dropped"), rig.rename.dropped));
                    out.push(format!(
                        "{:<10}{}",
                        r("$ap_inferred"),
                        rig.out.infer.added.len()
                    ));
                    out.push(format!(
                        "{:<15}{}",
                        r("$ap_limbs_aligned"),
                        rig.out.reorient.limbs_aligned
                    ));
                    let edited = rig.offsets.iter().filter(|o| !o.is_zero()).count();
                    out.push(format!("{:<15}{edited}", r("$ap_bones_edited")));
                    if !rig.rename.unmapped.is_empty() {
                        out.push(format!(
                            "{} {}",
                            r("$ap_unmapped"),
                            rig.rename.unmapped.join(", ")
                        ));
                    }
                }
            },
            "attach" => {
                let resolved = (0..src.attach.len())
                    .filter(|i| self.attach_resolved(*i))
                    .count();
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
                out.push(format!(
                    "{:<11}{}",
                    r("$ap_class_word"),
                    class_label(src.class())
                ));
                match src.parsed.as_ref() {
                    Some(p) => {
                        out.push(format!(
                            "{:<11}{} {}",
                            r("$ap_skeleton"),
                            p.bones(),
                            r("$ap_bones")
                        ));
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
                            format!(
                                "{} {TRI_BUDGET}{}",
                                r("$ap_within_the"),
                                r("$ap_tri_budget")
                            )
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
        let Some(src) = self.source.as_ref() else {
            return String::new();
        };
        if src.error.is_some() {
            return r("$ap_badge_blocked");
        }
        match self.wf.step() {
            "conform" if matches!(src.class(), Some(AssetClass::Prop | AssetClass::Animation)) => {
                match src.class() {
                    Some(AssetClass::Animation) => r("$ap_badge_clip"),
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
        let Some(src) = self.source.as_mut() else {
            return;
        };
        src.parsed = None;
        src.report = None;
        // Keep the DECLARED workflow: the next piece in a set is the same class the user chose on
        // the Workflow page, not a re-detection.
        src.class = self.pending_class;
        src.rig = None;
        src.committed = None;
        src.error = None;
        src.fit = PropFit::default();
        src.clip = None;
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
            Some(AssetClass::Prop) => WF_PROP,
            Some(AssetClass::Animation) => WF_ANIMATION,
            Some(AssetClass::Skin) | None => WF_CHARACTER,
        };
        match self.workflows.get(name) {
            Some(def) => {
                self.wf_def = def.clone();
                self.wf = Workflow::from_def(&self.wf_def);
            }
            // `new()` proved the file loads, so this is a definition NAME drifting from the
            // content — warn and keep the running wizard rather than losing the user's place.
            None => tracing::warn!(
                "workflow: no `{name}` in ui_workflows.json — keeping the current one"
            ),
        }
    }

    /// This frame's step DOCUMENT — the workflow runtime's gating input, the `needs` /
    /// `yields` vocabulary of `ui_workflows.json`. A key is PRESENT exactly when the scene
    /// state it names exists; values are incidental (the runtime only probes presence).
    fn wf_doc(&self) -> ValueMap {
        let mut d = ValueMap::new();
        let Some(src) = self.source.as_ref() else {
            return d;
        };
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
        // The mount binding applies to a Prop — but only once its MESH actually parsed.
        // Unconditional, this let a folder with nothing loadable walk to Review and
        // "export" nothing (the silent-commit repro from the QA pass).
        if src.class() == Some(AssetClass::Prop) && src.parsed.is_some() {
            d.set("fit", true);
        }
        // The Animation gate: the active BVH retargeted and previewable. Present exactly
        // when `prepare_clip` succeeded — a failed retarget leaves the wizard at the Clip
        // page with the error in the inspector, fail-loud.
        if src.clip.is_some() {
            d.set("clip", true);
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
        let Some(src) = self.source.as_mut() else {
            return false;
        };
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
                    // The clip preview derives from the ACTIVE pick; the conform-step
                    // runner (`prepare_clip`) rebuilds it next frame, like analyze/conform.
                    src.clip = None;
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
                let slot = if i < 3 {
                    &mut src.fit.offset[i]
                } else {
                    &mut src.fit.rot[i - 3]
                };
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
        let Some(rig) = src.rig.as_mut() else {
            return edited;
        };
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
        let Some(src) = self.source.as_ref() else {
            return (Vec::new(), Vec::new());
        };
        let step = self.wf.step();
        if step != "attach" && step != "review" {
            return (Vec::new(), Vec::new());
        }
        let r = radius * 0.04;
        let (mut plain, mut sel) = (Vec::new(), Vec::new());
        for i in 0..src.attach.len() {
            let Some(c) = self.attach_world(i) else {
                continue;
            };
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
// THE canon count — read from the authored baseline table, never restated.
const REFERENCE_BONES: usize = flicker_content::baseline::CANON_BONES;

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
const OFFSET_AXES: [(&str, &str); 4] = [
    ("off_x", "Translate X"),
    ("off_y", "Translate Y"),
    ("off_z", "Translate Z"),
    ("off_roll", "Roll"),
];

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
const FIT_SCALE_AXES: [(&str, &str); 3] = [
    ("fit_sx", "Scale X"),
    ("fit_sy", "Scale Y"),
    ("fit_sz", "Scale Z"),
];

/// A batch of world-space line segments, in the shape `Renderer::draw_lines_overlay` takes.
type Segments = Vec<(Vec3, Vec3)>;

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
            globals.get(b).copied().unwrap_or(Mat4::IDENTITY)
                * Mat4::from_cols_array(&bone.inverse_bind)
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
                let m = palette
                    .get(v.joints[k] as usize)
                    .copied()
                    .unwrap_or(Mat4::IDENTITY);
                pos += w * (m * p).truncate();
                nrm += w * (glam::Mat3::from_mat4(m) * n);
            }
            let position = if any { pos.to_array() } else { v.p };
            let normal = if nrm.length_squared() > 1e-12 {
                nrm.normalize().to_array()
            } else {
                v.n
            };
            MeshVertex {
                position,
                normal,
                material: 0,
            }
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
        let q =
            glam::Quat::from_array(b.rotation) * glam::Quat::from_rotation_x(o.roll.to_radians());
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
    const PAIRS: [(&str, &str); 6] = [
        ("_l", "_r"),
        ("_r", "_l"),
        ("_L", "_R"),
        ("_R", "_L"),
        (".L", ".R"),
        (".R", ".L"),
    ];
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

/// Where committed CLIP VARIANTS land — the shared retarget library's STAGING tier,
/// mirroring the package layout the paperdoll reads (`retarget/clips/<set>/…`), reached
/// by promotion exactly like [`characters_dir`]'s rigs.
fn clips_dir() -> PathBuf {
    flicker_content::roots()
        .staging()
        .join("retarget")
        .join("clips")
}

/// Where committed ENVIRONMENT props land — their own staging tier (`staging/props/`):
/// a tree filed under `characters/` would read as classified nonsense in the
/// Quartermaster. Promoted into the package like everything else.
fn props_dir() -> PathBuf {
    flicker_content::roots().staging().join("props")
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
    (
        centre,
        ((hi - lo).max_element() * 0.5).max(1.0),
        lo.z - centre.z,
    )
}

/// Build the Clayworks Bench editor as a boxed [`Scene`] — the CLIENT BEHAVIOUR the
/// roster registers; the manifest resolves `assetpipeline.scene.json` and hands its
/// def here.
pub fn scene(def: &SceneDef) -> Box<dyn Scene> {
    Box::new(AssetPipeline::new(def))
}

impl Scene for AssetPipeline {
    fn enter(&mut self, renderer: &mut Renderer) {
        // 1×1 white pixel — `render_hud` tints it to build the HUD's solid quads.
        self.hud_white = Some(renderer.load_texture(&[0xff, 0xff, 0xff, 0xff], 1, 1));
        // The shared 2×2 editor viewport (Perspective TL, Top TR, Side BL, Front BR) —
        // the same grid the paperdoll uses, owned by flicker-render.
        self.grid = Some(QuadGrid::editor(renderer));
        self.clip_grid = Some(QuadGrid::new(renderer, &CLIP_VIEWS, 2));
        self.bake_grid = Some(QuadGrid::new(renderer, &BAKE_VIEWS, 1));
        // Built once and handed to each PauseScene we push, so pausing never re-uploads.
        self.ui_theme = Some(Theme::build(renderer));
        // PRE-LOAD the fitting body (the clay Golem) WITH the scene — mesh and all — so turning
        // the reference body on for a prop or an outfit is instant instead of a first-fit hitch.
        self.base = BasePreview::load();
        // Split the borrows: the loaded body is READ while the texture cache is filled.
        let Self {
            base,
            textures,
            base_upload,
            ..
        } = self;
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

    fn update(
        &mut self,
        dt: Duration,
        input: &InputState,
        signals: &mut SceneInput,
        renderer: &Renderer,
    ) -> Transition {
        // The HUD walks first: it lays out the framed holder, and the rect it reserves for the 2×2 is
        // what the viewport controls below pick against — a cursor outside the holder (over the
        // inspector or a bar) lands on no view, so the chrome never has to "claim" the pointer.
        let screen = renderer.size();
        let Some(tree) = self.authored.take() else {
            return Transition::None;
        };
        // The scene is DATA: walk the AUTHORED tree with the raw model + the pair
        // script's derived presentation (rail-chip styles, bank washes).
        let model = self.model();
        let snap = UiInput {
            mouse: input.mouse_position,
            clicked: input.mouse_left_pressed,
            down: input.mouse_left,
            screen,
            typed: String::new(),
            backspace: false,
            wheel: input.mouse_wheel_delta,
        };
        let frame = run_ui(&tree, &model, &self.ui_styles, &snap, &mut self.ui_state);
        // The framed holder the HUD reserved for the 2×2 (the `editor_quad` rtt): the grid
        // tiles inside exactly this rect, so the four views land in the frame and the
        // inspector sits beside them. Setting it on the grid keeps the composite and the
        // pointer-picking in step. (Read before `commands` is moved out of the frame.)
        let viewport = frame.rtt_rect("editor_quad");
        self.hud_commands = frame.commands;
        self.quad_rect = viewport;
        if let Some(g) = self.grid.as_mut() {
            g.set_viewport(viewport);
        }
        // The clip pair and the bake preview tile in the SAME holder rect — whichever
        // grid renders this frame, the panels land inside the frame the HUD reserved.
        // (An unconfined grid tiles the WHOLE WINDOW and composites over the HUD —
        // the dead-page bug the preview page shipped with.)
        if let Some(g) = self.clip_grid.as_mut() {
            g.set_viewport(viewport);
        }
        if let Some(g) = self.bake_grid.as_mut() {
            g.set_viewport(viewport);
        }
        let hud_hit = frame.results.is_on("hud_hit");

        // ── The input seam (input-P3): the PUMP resolved this frame's events — the
        // scene owns no Resolver. One dispatch through [walker, editor]: the walker
        // owns the focus graph (panel cursor + in-pane nav), consumes the pointer
        // while it is over the HUD, and fires the screen's DECLARED intents
        // (`on_menu` / `on_tab_*` / `on_mode_*`) as result names; the editor layer
        // BELOW it owns the six camera signals exactly while the viewport pane is
        // ENTERED (the populous world-below-walker pattern). ──
        self.editor.owns_camera = self.ui_state.entered_group() == Some(VIEW_PANE);
        let owns_camera = self.editor.owns_camera;
        let mut walker = WalkerHandler::hud(&mut self.ui_state, hud_hit)
            .with_nav(&tree, &model)
            // The resolved rects make the panel tier move GEOMETRICALLY — Left from
            // the inspector lands on the viewport beside it (flatten, 1A292918).
            .with_rects(&frame.rects)
            .with_intents(&self.ui_intents);
        {
            let mut chain: [&mut dyn InputHandler; 2] = [&mut walker, &mut self.editor];
            Router::dispatch(signals.events, &mut chain, signals.route);
        }
        // The screen's fired intents (S9), drained once: folded into the results so
        // both input channels reach the ONE dispatch identically, and queued for the
        // one-frame `sig_<name>` Model mirror.
        self.fired_sigs = walker.take_fired();
        self.authored = Some(tree);

        let mut results = frame.results.clone();
        for name in &self.fired_sigs {
            results.set(name.clone(), true);
        }

        // ── The ONE dispatch: a click and a pad press arrive here identically. ──
        // The display toggles live on an always-walked panel, so their reads are
        // live every frame; the per-stage toggles stay gated by their pages.
        self.show_skeleton = results.is_on("show_skeleton");
        self.show_base = results.is_on("show_base");
        self.show_collision = results.is_on("show_collision");
        self.apply_gizmo_results(&results);
        // Joint symmetry — read only on Conform, where the checkbox lives (off the stage `is_on`
        // reads false and would clear it). Bake Skin re-weights the mesh to the repositioned rig.
        if self.wf.step() == "conform" {
            self.mirror_joints = results.is_on("mirror");
        }
        // The staged-reload preference — read only on Task, where its checkbox lives (same
        // page-gating rule as `mirror`: off the stage `is_on` reads false and would clear it).
        if self.wf.step() == "task" {
            self.prefer_staged = results.is_on("prefer_staged");
            self.as_provided = results.is_on("as_provided");
        }
        // The variant pick — read only on the Clip page, where its checkboxes live
        // (off the stage `is_on` reads false and would clear the choice).
        if self.wf.step() == "conform" && self.conform_role() == ConformRole::Clip {
            self.variant_rm = results.is_on("variant_rm");
            self.variant_ip = results.is_on("variant_ip");
        }
        if results.is_on("bake_skin") {
            self.bake_skin_now();
        }

        // The Task page's four workflow cards. Each DECLARES the class (+ Prop
        // sub-type) the user is importing, never auto-detected, then opens the OS
        // folder dialog IMMEDIATELY — the card IS the trigger, there is no separate
        // Load button. On a successful pick the existing `open()` runs
        // parse+conform+land inline (Character → the gizmo rig view); a CANCELLED
        // pick leaves `source == None`, so we fall back to the panel.
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

        // The screen DECLARED `on_menu = "pause_open"` (S9): the walker layer consumed
        // the Menu press and fired the name; the scene maps it onto the shell pause
        // overlay (Resume / Settings / Main Menu / Quit), the only supported way back
        // out of a scene. The pause overlay shows the PROFILE's map — the pump owns
        // bindings now (input-P3), the scene holds none.
        if results.is_on("pause_open") {
            if let Some(theme) = self.ui_theme {
                let pause_map = flicker_shell::input_profile()
                    .context_map("World")
                    .cloned()
                    .unwrap_or_else(InputMap::wasd_and_mouse);
                return Transition::Push(Box::new(PauseScene::new(
                    theme,
                    &pause_map,
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
            // The Animation sibling: retarget the active BVH once, then let the shared
            // clock run — both panels sample the same tick, wrapped at the clip length.
            self.prepare_clip();
        }
        if let Some(cp) = self.source.as_ref().and_then(|s| s.clip.as_ref()) {
            let hz = cp.ip.tick_rate_hz.max(1) as f32;
            self.clip_tick = (self.clip_tick + dt.as_secs_f32() * hz) % cp.duration as f32;
        }
        // The Preview page's playback clock — the baked body loops the shared idle.
        if let Some(bp) = self.bake.as_ref() {
            let hz = bp.clip.tick_rate_hz.max(1) as f32;
            self.bake_tick =
                (self.bake_tick + dt.as_secs_f32() * hz) % bp.clip.duration_ticks.max(1) as f32;
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
        // THE PREVIEW PAGE takes the shared Orbit component's standard pointer contract on
        // its single view — the same camera interaction every body view owes (rule: connect
        // the ratified system, never route around it). The quad block below stays the quad's.
        let on_preview = self.wf.step() == "preview";
        if on_preview && self.ui_state.drag().is_none() {
            if let Some(bp) = self.bake.as_ref() {
                let inside = self
                    .bake_grid
                    .as_ref()
                    .and_then(|g| g.cell_at(input.mouse_position, screen))
                    .is_some();
                if inside {
                    let view_h = self.quad_rect.map(|r| r.size.y).unwrap_or(screen.y);
                    self.bake_orbit.apply_pointer(
                        delta,
                        input.mouse_left,
                        input.mouse_right,
                        input.mouse_wheel_delta,
                        bp.radius,
                        view_h,
                    );
                }
            }
        }
        if !on_preview && self.ui_state.drag().is_none() {
            if let Some(i) = self
                .grid
                .as_ref()
                .and_then(|g| g.cell_at(input.mouse_position, screen))
            {
                // Orbit (left-drag) is a PERSPECTIVE notion — only the PERSP panel (view 0) rotates;
                // the ortho views are fixed axes. A flip-label click is not an orbit, and neither is
                // a gizmo pick/drag.
                if input.mouse_left && flip_target.is_none() && i == 0 && !gizmo {
                    self.orbits[0].yaw -= delta.x * 0.006;
                    self.orbits[0].pitch =
                        (self.orbits[0].pitch + delta.y * 0.006).clamp(-1.4, 1.4);
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

        // Continuous camera queries against the PUMP's active-context bindings
        // (input-P3): `signals.axis` unifies held keys and stick deflection into ONE
        // 0..1 path per look signal. The editor answers them only while its pane is
        // ENTERED — the same gate that let the [`EditorLayer`] consume the discrete
        // edges above — and they drive the PERSPECTIVE panel (the ortho views are
        // fixed axes; the pointer pans them).
        if owns_camera {
            let dt_s = dt.as_secs_f32();
            let look = Vec2::new(
                signals.axis(ActionSignal::LookRight, input)
                    - signals.axis(ActionSignal::LookLeft, input),
                signals.axis(ActionSignal::LookUp, input)
                    - signals.axis(ActionSignal::LookDown, input),
            );
            let zoom = signals.axis(ActionSignal::ZoomIn, input)
                - signals.axis(ActionSignal::ZoomOut, input);
            // The signals drive whichever perspective view the step shows: the preview
            // page's single bake view, else the quad's PERSP panel — same contract, one
            // component (controller is the floor on every body view).
            let o = if on_preview {
                &mut self.bake_orbit
            } else {
                &mut self.orbits[0]
            };
            if look != Vec2::ZERO {
                o.yaw -= look.x * STICK_LOOK_RATE * dt_s;
                o.pitch = (o.pitch + look.y * STICK_LOOK_RATE * dt_s).clamp(-1.4, 1.4);
            }
            if zoom != 0.0 {
                o.zoom_by(zoom * STICK_ZOOM_RATE * dt_s);
            }
        }

        Transition::None
    }

    fn render(&mut self, renderer: &mut Renderer) {
        let base_layer = renderer.layer();

        // THE CLIP PAGE swaps the 2×2 rig grid for the side-by-side variant pair.
        // Exactly ONE grid drives the frame graph per frame — its offscreen passes
        // reset the per-frame draw queues, so both running would discard each other.
        let clip_active = self.wf.step() == "conform"
            && self.conform_role() == ConformRole::Clip
            && self
                .source
                .as_ref()
                .map(|s| s.clip.is_some())
                .unwrap_or(false);
        if clip_active {
            self.render_clip(renderer, base_layer);
        }
        // THE PREVIEW PAGE swaps the rig grid for the baked-idle smoke test — the same
        // one-grid-per-frame exclusivity rule as the clip pair above. If the bake FAILED,
        // fall through to the normal rig grid (the inspector shows the error) — the page
        // must never be a dead frame with nothing rendering.
        let bake_active = if self.wf.step() == "preview" {
            self.ensure_bake_preview(renderer);
            if self.bake.is_some() {
                self.render_bake_preview(renderer, base_layer);
                true
            } else {
                false
            }
        } else {
            if let Some(bp) = self.bake.take() {
                // Leaving the page drops the bake, so re-entry rebuilds from the latest joints.
                renderer.free_skinned_mesh(bp.mesh);
            }
            false
        };

        // The four views FIRST. `QuadGrid::render` drives the shared `FrameGraph`, whose
        // offscreen passes RESET the per-frame draw queues (the centralized "render RTTs
        // before the main view" rule) — so anything queued before this is discarded, and
        // the HUD must come after. The views composite one layer above the backdrop.
        // The skeleton is drawn as an overlay so it reads through anything in front of it.
        // Prop/garment fit preview (the base rig + the imported mesh at socket·fit). Built FIRST —
        // its mesh upload needs `&mut Renderer` before grid.render borrows it for the RTT passes.
        let preview = if clip_active || bake_active {
            None
        } else {
            self.ensure_preview(renderer)
        };
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

        if let (false, Some(grid)) = (clip_active || bake_active, self.grid.as_ref()) {
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
                .map(|i| {
                    grid.camera(
                        i,
                        self.orbits[i].ortho_radius(radius),
                        &self.orbits[i].camera(radius),
                    )
                })
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
                    let radii = debug::joint_ball_radii(
                        &p.parents,
                        &p.globals,
                        BALL_LEN_FRAC,
                        min_r,
                        max_r,
                    );
                    let bones =
                        debug::bone_diamonds(recentre, &p.parents, &p.globals, BONE_WAIST_FRAC);
                    let mut balls = Segments::new();
                    for (i, g) in p.globals.iter().enumerate() {
                        if Some(i) == sel {
                            continue; // the focused joint draws amber below
                        }
                        let c = recentre.transform_point3(g.w_axis.truncate());
                        balls.extend(debug::wireframe(&Shape::Sphere {
                            center: c,
                            radius: radii[i],
                        }));
                    }
                    // The FOCUSED joint: a larger amber ball (the grab handle) + the PAN gizmo. The
                    // gizmo is drawn ONLY in the ortho panels (see the draw loop) — Perspective grabs
                    // the ball for the spring-back deform test. The GIZMO MODE toggle picks the handle
                    // shape (translate now; rotate/scale are the seam for later pipeline stages).
                    let (sel_ball, gizmo_groups) =
                        match sel.and_then(|s| p.globals.get(s).map(|g| (s, g))) {
                            Some((s, g)) => {
                                let c = recentre.transform_point3(g.w_axis.truncate());
                                let r = radii.get(s).copied().unwrap_or(0.5) * 1.4;
                                let ball = debug::wireframe(&Shape::Sphere {
                                    center: c,
                                    radius: r,
                                });
                                let size = (radius * GIZMO_ARROW_FRAC).max(1.0);
                                let lines = gizmo_segments(
                                    c,
                                    glam::Mat3::IDENTITY,
                                    self.gizmo_mode,
                                    size,
                                    None,
                                );
                                (ball, group_by_color(lines))
                            }
                            None => (Segments::new(), Vec::new()),
                        };
                    (bones, balls, sel_ball, gizmo_groups)
                }
                _ => (
                    Segments::new(),
                    Segments::new(),
                    Segments::new(),
                    Vec::new(),
                ),
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
                s.into_iter()
                    .map(|(a, b)| (a - centre, b - centre))
                    .collect()
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
                // Skeleton rig-view: violet octahedral diamond bones under cyan per-joint scaled
                // balls — two hues, so structure and grab points read apart at a glance.
                if !bones.is_empty() {
                    r.draw_lines_overlay(&bones, BONE);
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
                        r.draw_lines_overlay(&pv.base_joints, BONE);
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
    use flicker::render::ORBIT_FOV_Y;

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
        assert_eq!(
            ed.wf.step(),
            step,
            "`{step}` is ahead of here in the active workflow"
        );
    }

    /// An editor with the real CHARACTER asset loaded, parsed and conformed — parked
    /// exactly where `open` lands it: the `conform` step of the character workflow.
    /// `open` dispatches the definition and runs ingest → parse → conform inline
    /// (exactly as clicking the Import Character card does), so the derived state
    /// exists just as `update` would leave it.
    fn at_conform() -> Option<AssetPipeline> {
        let dir = real_source()?;
        let mut ed = AssetPipeline::shipped();
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
        assert!(
            !parsed.collision.is_empty(),
            "auto-fit produced collision volumes for the character"
        );
        let capsules = parsed
            .collision
            .iter()
            .filter(|v| matches!(v.shape, Shape::Capsule { .. }))
            .count();
        let spheres = parsed
            .collision
            .iter()
            .filter(|v| matches!(v.shape, Shape::Sphere { .. }))
            .count();
        assert!(
            capsules > 0,
            "bones with children fit capsules (the collision boxes)"
        );
        assert!(
            spheres > 0,
            "leaf bones (fingertips/toes/head end) fit spheres (the joint balls)"
        );
        assert!(
            parsed
                .collision
                .iter()
                .all(|v| v.bone < parsed.globals.len()),
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
        let bones = json["skeleton"]["bones"]
            .as_array()
            .expect("skeleton.bones")
            .len();
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
        assert_eq!(
            parsed.bones(),
            CONFORMED_BONES,
            "conform reaches the canonical bone count"
        );
        assert_eq!(
            rig.map.len(),
            parsed.bones(),
            "one provenance per bone, no gaps"
        );
        let (ok, review, auto) = rig.counts();
        assert_eq!(
            ok + review + auto,
            parsed.bones(),
            "the buckets partition the skeleton"
        );
        assert_eq!(
            auto,
            rig.out.infer.added.len(),
            "auto is exactly what infer added — not a recount"
        );
        assert!(
            review > 0,
            "the hip/shoulder/ankle derives flagged joints for review"
        );
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

        rig.offsets[sel] = BoneOffset {
            t: [0.0, 0.0, 7.0],
            roll: 0.0,
        };
        let offsets = rig.offsets.clone();
        src.parsed.as_mut().unwrap().rebuild(&offsets);
        let after = src.parsed.as_ref().unwrap().globals.clone();

        assert_ne!(
            before[sel].w_axis, after[sel].w_axis,
            "the edited bone moved"
        );
        let head = src.parsed.as_ref().unwrap().bone_index("head").unwrap();
        assert_ne!(
            before[head].w_axis, after[head].w_axis,
            "the offset propagated to children"
        );

        // Reset → identical frames, bit for bit.
        let src = ed.source.as_mut().unwrap();
        src.rig.as_mut().unwrap().offsets[sel] = BoneOffset::default();
        let offsets = src.rig.as_ref().unwrap().offsets.clone();
        src.parsed.as_mut().unwrap().rebuild(&offsets);
        assert_eq!(
            src.parsed.as_ref().unwrap().globals,
            before,
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
            assert!(
                rig.sel >= rig.window && rig.sel < rig.window + BONE_ROWS,
                "cursor stays visible"
            );
            if rig.sel + 1 >= total {
                break;
            }
        }
        assert!(
            seen >= total,
            "paging reached the last bone ({seen} of {total})"
        );

        // And back again, without overshooting the top.
        for _ in 0..total {
            ed.apply_stage_results(&ValueMap::new().with("bone_prev", true));
        }
        let rig = ed.source.as_ref().unwrap().rig.as_ref().unwrap();
        assert_eq!(
            (rig.sel, rig.window),
            (0, 0),
            "paging back lands on the first bone"
        );

        // Selecting a row off-window scrolls it into view.
        let rig = ed.source.as_mut().unwrap().rig.as_mut().unwrap();
        rig.sel = total - 1;
        rig.window = 0;
        AssetPipeline::scroll_to_selection(rig);
        assert!(
            rig.sel >= rig.window && rig.sel < rig.window + BONE_ROWS,
            "selection is visible"
        );
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
        assert!(
            (after.x - before.x - 5.0).abs() < 1e-4,
            "{before} → {after}"
        );
        assert_eq!(
            ed.attach_world(3).unwrap(),
            ed.attach_world(3).unwrap(),
            "others unmoved"
        );

        // Markers only exist on the stages that authored them. Back is clean here — the
        // direct `apply_stage_results` call above bypassed `update`, so nothing armed the
        // discard guard and the runtime steps straight back.
        assert!(
            !ed.attach_markers(100.0).0.is_empty(),
            "markers drawn on the Attach stage"
        );
        ed.apply_workflow_results(&fired("wf_back"));
        assert_eq!(ed.wf.step(), "conform");
        assert!(
            ed.attach_markers(100.0).0.is_empty(),
            "and not on the rig stage before it"
        );
    }

    /// REVIEW: every requirement is computed from real state. With the real asset conformed they
    /// all pass; with nothing loaded there is nothing to claim.
    #[test]
    fn review_requirements_read_the_real_state() {
        let empty = AssetPipeline::shipped();
        assert!(empty.requirements().is_empty(), "no asset → no claims");

        let Some(mut ed) = at_conform() else {
            eprintln!("skipping: no content tree");
            return;
        };
        park(&mut ed, "review");
        let reqs = ed.requirements();
        assert_eq!(
            reqs.len(),
            REQUIREMENT_ROWS,
            "the tree declares this many rows"
        );
        for (ok, text) in &reqs {
            assert!(ok, "requirement failed on the reference asset: {text}");
        }

        // A requirement is a real gate: break one and Commit must go dark.
        let m = ed.hud_model();
        assert!(m.is_on("commit_enabled"));
        ed.source.as_mut().unwrap().textures = 0;
        assert!(
            !ed.hud_model().is_on("commit_enabled"),
            "a failed check blocks commit"
        );
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
        assert!(
            !ed.wf.can_next(&ed.wf_doc()),
            "Conform has not produced a rig yet"
        );
        ed.apply_workflow_results(&fired("wf_next"));
        assert_eq!(
            ed.wf.step(),
            "conform",
            "the refused advance holds the page"
        );
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
        let mut ed = AssetPipeline::shipped();
        // The Import Prop card's declaration — `open` dispatches the prop workflow with it.
        ed.pending_class = Some(AssetClass::Prop);
        ed.open(dir);
        assert_eq!(ed.wf.step(), "conform", "a prop lands on its Mount page");
        let src = ed.source.as_ref().unwrap();
        assert!(
            src.rig.is_none(),
            "the character conform path must not run on a prop"
        );
        assert!(
            src.error.is_none(),
            "and it must NOT invent a skeleton failure"
        );
        // A prop SKIPS the character-only pages and walks on to commit its mesh directly —
        // its definition's next stop is Review, whose `fit` need the mount binding meets.
        assert!(
            ed.wf.can_next(&ed.wf_doc()),
            "a prop walks past the character-only conform"
        );
        assert_eq!(
            ed.inspector_badge(),
            "STATIC",
            "a non-clothing prop bakes as a static mesh"
        );
        let lines = ed.inspector_lines();
        assert!(
            lines
                .iter()
                .any(|l| l.to_ascii_lowercase().contains("prop")),
            "the stage names the detected class: {lines:?}"
        );
        assert!(
            lines
                .iter()
                .any(|l| l.to_ascii_lowercase().contains("paperdoll")),
            "and says the socket/fit are authored in the paperdoll: {lines:?}"
        );
    }

    /// THE STAGED-RELOAD PATH (Aaron 2026-08-20): with a staged rig present under the asset's
    /// name, `adopt_staged_from` replaces the parse+conform with the staged model loaded
    /// ALREADY-CONFORMED — every bone-map row Ok, zero offsets, no rename — so the wizard
    /// lands on the rig view holding exactly what was last committed, ready to adjust
    /// further. Exercised against a scratch root, like the commit path.
    #[test]
    fn adopt_staged_reopens_the_committed_rig() {
        let Some(mut ed) = at_conform() else {
            eprintln!("skipping: no content tree");
            return;
        };
        // Stage a small baked rig under this asset's name in a scratch root.
        let name = ed.source.as_ref().unwrap().asset_name().to_string();
        let scratch = std::env::temp_dir().join("flicker_assetpipeline_adopt_staged");
        let _ = std::fs::remove_dir_all(&scratch);
        let staged = {
            let parsed = ed.source.as_ref().unwrap().parsed.as_ref().unwrap();
            flicker_content::bake_rig(&parsed.model, &name)
        };
        let dir = scratch.join(&name);
        std::fs::create_dir_all(&dir).expect("scratch dir");
        std::fs::write(
            dir.join(format!("{name}.json")),
            serde_json::to_string(&staged).expect("staged rig serializes"),
        )
        .expect("staged rig writes");
        let staged_bones = staged.skeleton.bones.len();

        // Wipe the conform result and adopt: the rig must come back pre-filled from the file.
        {
            let s = ed.source.as_mut().unwrap();
            s.parsed = None;
            s.rig = None;
        }
        assert!(ed.adopt_staged_from(&scratch, "staging"), "the staged rig adopts");
        let src = ed.source.as_ref().unwrap();
        assert_eq!(
            src.reopened,
            Some("staging"),
            "provenance names where the rig came from"
        );
        let parsed = src.parsed.as_ref().expect("the staged model is parsed-in");
        assert_eq!(
            parsed.bones() + 1,
            staged_bones,
            "the synthesized root strips back off on the way in"
        );
        let rig = src.rig.as_ref().expect("the staged model loads already-conformed");
        assert!(
            rig.map.iter().all(|s| *s == MapState::Ok),
            "every staged bone carries Ok provenance"
        );
        assert_eq!(rig.rename.renamed, 0, "a staged rig is already canonical");
        assert_eq!(
            rig.offsets.len(),
            parsed.bones(),
            "offset rows parallel the staged skeleton"
        );
        assert!(
            rig.out.reorient.limbs_aligned == 0,
            "no conform pass ran over the human's fitted joints"
        );

        // A MISSING staged rig falls through: state stays untouched, ready for the next root
        // in the staging→package search order (the promote-emptied-staging case Aaron hit —
        // the ONE copy of a promoted fit lives in package).
        {
            let s = ed.source.as_mut().unwrap();
            s.parsed = None;
            s.rig = None;
            s.reopened = None;
        }
        let empty = std::env::temp_dir().join("flicker_assetpipeline_adopt_staged_empty");
        let _ = std::fs::remove_dir_all(&empty);
        assert!(
            !ed.adopt_staged_from(&empty, "staging"),
            "an empty root adopts nothing"
        );
        {
            let src = ed.source.as_ref().unwrap();
            assert!(
                src.parsed.is_none() && src.rig.is_none() && src.reopened.is_none(),
                "no staged rig → the FBX path stays in charge"
            );
        }
        // …and the same scratch rig offered as the PACKAGE root adopts with its provenance.
        assert!(
            ed.adopt_staged_from(&scratch, "package"),
            "the promoted copy adopts when staging is empty"
        );
        assert_eq!(ed.source.as_ref().unwrap().reopened, Some("package"));
        let _ = std::fs::remove_dir_all(&scratch);
    }

    /// THE FIT-RETENTION PROOF (Aaron 2026-08-20: "It is unclear if the repositioned skeleton
    /// layout is retained. Check that." — skips without the promoted golem): re-opening the
    /// PROMOTED rig keeps every fitted joint's WORLD position exactly — through the load, the
    /// chain splice, and a re-bake — and the fitted signature (the hand-tuned depths, NOT the
    /// canonical reference positions) is what comes back.
    #[test]
    fn adopting_the_promoted_golem_retains_the_fitted_joints() {
        let path = flicker_content::roots()
            .package()
            .join("characters/GolemBase_Low/GolemBase_Low.json");
        if !flicker_content::package::file_exists(&path) {
            eprintln!("skipping: no promoted golem");
            return;
        }
        let mut m = flicker_content::load_rig_raw(&path).expect("promoted golem loads");
        let world = |m: &flicker_content::RawModel| -> std::collections::HashMap<String, Vec3> {
            let (globals, _) = rest_globals(m, &[]);
            m.bones
                .iter()
                .zip(&globals)
                .map(|(b, g)| (b.name.clone(), g.w_axis.truncate()))
                .collect()
        };
        let before = world(&m);
        // The AUTHORED signature, not canon: this body's hands sit far inboard of the
        // canonical 63.9 (the golem's own proportions) — if a conform pass had run over the
        // reload, they would snap back toward canon. The head is deliberately NOT pinned:
        // it is the joint the human keeps re-authoring, so freezing one fit's value here
        // made the guard fail on every legitimate re-promote.
        assert!(
            before["hand_l"].x < 50.0,
            "the promoted rig carries the AUTHORED hand, got {}",
            before["hand_l"]
        );
        // The chain heal moves NO joint: world frames are preserved by construction.
        let spliced =
            flicker_content::splice_canonical_chain(&mut m, &flicker_content::default_reference())
                .expect("splice runs");
        let after = world(&m);
        for (name, p) in &before {
            let q = after[name];
            assert!(
                (q - *p).length() < 1e-2,
                "splice moved {name}: {p} → {q} (spliced={spliced:?})"
            );
        }
        // …and a re-commit round trip returns the same skeleton, byte-shaped.
        let baked = flicker_content::bake_rig(&m, "GolemBase_Low");
        let m2 = {
            // rig_to_raw is crate-private to flicker-content; round-trip through serde instead.
            let text = serde_json::to_string(&baked).expect("bake serializes");
            let tmp = std::env::temp_dir().join("flicker_assetpipeline_fit_retention");
            let _ = std::fs::remove_dir_all(&tmp);
            std::fs::create_dir_all(tmp.join("X")).expect("tmp");
            std::fs::write(tmp.join("X/X.json"), text).expect("write");
            let m2 = flicker_content::load_rig_raw(&tmp.join("X/X.json")).expect("reload");
            let _ = std::fs::remove_dir_all(&tmp);
            m2
        };
        let rebaked = world(&m2);
        for (name, p) in &after {
            let q = rebaked[name];
            assert!(
                (q - *p).length() < 1e-2,
                "re-bake moved {name}: {p} → {q}"
            );
        }
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
        assert!(
            src.error.is_none(),
            "the prop commit succeeds: {:?}",
            src.error
        );
        let out = src.committed.clone().expect("a committed path is recorded");
        let text = flicker_content::package::read_text(&out).expect("the prop rig was written");
        assert!(
            text.contains("\"bones\":[]"),
            "a prop bakes an EMPTY skeleton: {out:?}"
        );
        assert!(
            text.contains("\"retarget\":false"),
            "a prop is retarget:false"
        );

        // And it SHIPS ITS TEXTURES: the wizard hands the bake the source mesh, so the vendor's maps
        // are copied beside the rig under the content standard's names and referenced by the
        // material — a prop that arrives as a lone `.json` renders untextured.
        let name = ed.source.as_ref().unwrap().asset_name().to_string();
        let dir = out.parent().expect("the rig sits in the asset's folder");
        let rig: flicker_skeletal::format::RigFile =
            serde_json::from_str(&text).expect("the prop rig parses");
        let m = rig.mesh.materials.first().expect("the prop has a material");
        assert_eq!(
            m.base_color,
            format!("{name}_BaseColor.png"),
            "albedo wired into the material"
        );
        assert!(
            dir.join(&m.base_color).exists(),
            "and copied beside the rig"
        );
        for map in [&m.normal, &m.roughness, &m.metalness] {
            assert!(
                !map.is_empty(),
                "every source map the standard has a slot for is wired"
            );
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
        let mut ed = AssetPipeline::shipped();
        ed.pending_class = Some(AssetClass::Prop); // the Import Accessory / Prop card's declaration
        ed.open(dir);
        {
            let src = ed.source.as_ref().expect("the folder opened");
            assert!(
                src.candidates.len() > 1,
                "a weapon set holds several meshes"
            );
            assert!(
                src.error.is_none(),
                "several meshes is a CHOICE, not an error: {:?}",
                src.error
            );
            assert_eq!(
                src.fbx, src.candidates[0],
                "the first is pre-selected — never stuck"
            );
        }
        assert_eq!(
            ed.wf.step(),
            "conform",
            "lands on the rig view, first piece ready"
        );
        assert!(
            ed.hud_model().is_on("on_pick"),
            "the inline picker is shown for a choice"
        );
        assert!(
            ed.source.as_ref().unwrap().parsed.is_some(),
            "the first pick is parsed on open"
        );

        // Choose the second — the stale parse must be dropped so nothing derived carries forward.
        ed.apply_stage_results(&ValueMap::new().with("pick_sel_1", true));
        let src = ed.source.as_ref().unwrap();
        assert_eq!(src.candidate_sel, 1);
        assert_eq!(
            src.fbx, src.candidates[1],
            "the import now points at the second mesh"
        );
        assert!(
            src.parsed.is_none(),
            "the previous mesh's parse was dropped"
        );
        assert!(
            src.report.is_none() && src.rig.is_none(),
            "and everything derived from it"
        );
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
        let mut ed = AssetPipeline::shipped();
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
        assert!(
            ed.hud_model().is_on("has_committed"),
            "the loop-back is offered"
        );

        let n = ed.source.as_ref().unwrap().candidates.len();
        ed.start_next_piece();
        assert_eq!(
            ed.wf.step(),
            "conform",
            "back on the rig view for the next piece"
        );
        let src = ed.source.as_ref().unwrap();
        assert_eq!(
            src.candidates.len(),
            n,
            "the folder and its piece list are kept"
        );
        assert!(
            src.parsed.is_none() && src.rig.is_none() && src.committed.is_none(),
            "the finished piece's state is dropped so the next starts clean"
        );
        assert!(
            ed.hud_model().is_on("on_pick"),
            "the inline picker is showing again"
        );
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
        assert_eq!(
            ed.wf.step(),
            "task",
            "Back from the rig view returns to the workflow page"
        );
        assert!(ed.source.is_some(), "which leaves the folder open");
        assert!(
            ed.hud_model().is_on("back_enabled"),
            "Back stays LIVE with a folder open — it clears it"
        );

        // Back again on the Workflow page (the first, with no earlier stop) clears the open folder.
        ed.apply_workflow_results(&fired("wf_back"));
        assert!(
            ed.source.is_none(),
            "Back on the workflow page clears the open folder"
        );
        assert!(
            !ed.hud_model().is_on("back_enabled"),
            "and then there is nothing to go back from"
        );
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
        let mut ed = AssetPipeline::shipped();
        // The Import Prop card's declaration; `open` dispatches the prop workflow, whose
        // rig page IS Conform under the Mount role — not a separate later stage.
        ed.pending_class = Some(AssetClass::Prop);
        ed.open(dir);
        assert_eq!(ed.wf.step(), "conform");

        // Conform DISPATCHES: a prop gets the mount panel, and NOT the character bone map that
        // used to leave this page empty for it.
        let m = ed.hud_model();
        assert_eq!(
            ed.conform_role(),
            ConformRole::Mount,
            "a prop conforms by mounting"
        );
        assert!(
            m.is_on("on_conform_mount"),
            "a prop authors its mount ON the conform page"
        );
        assert!(
            !m.is_on("on_conform_skeleton"),
            "and NOT the character bone map"
        );
        assert!(
            !m.is_on("on_conform_clip"),
            "and NOT the animation placeholder"
        );
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
        assert_eq!(
            fit.socket,
            window + 2,
            "the picked socket row is now the mount"
        );
        assert!(
            (fit.offset[0] - 3.5).abs() < 1e-4,
            "the offset slider authored the fit"
        );
        assert!(
            (fit.rot[2] - 45.0).abs() < 1e-4,
            "the rotation slider authored the fit"
        );
        // Per-axis scale RESHAPES (only the dragged axis moves) and scale-all is a SEPARATE
        // multiplier — the paperdoll gadget's pair. Conflating them would silently rescale the
        // other two axes the moment the user touched one.
        assert!(
            (fit.scale[1] - 2.0).abs() < 1e-4,
            "the Y scale slider authored that axis"
        );
        assert!((fit.scale[0] - 1.0).abs() < 1e-4, "and left X alone");
        assert!((fit.scale[2] - 1.0).abs() < 1e-4, "and left Z alone");
        assert!(
            (fit.uniform - 1.5).abs() < 1e-4,
            "scale-all rides `fit_scale`"
        );

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
        assert!(
            (baked.scale[1] - 2.0).abs() < 1e-4,
            "per-axis scale survives to the format"
        );
        assert!(
            (baked.uniform - 1.5).abs() < 1e-4,
            "scale-all survives to the format"
        );

        // And that authored socket is a REAL bone name the bake can resolve against the base.
        assert!(
            SOCKETS.get(fit.socket).is_some(),
            "the mount indexes the socket table"
        );
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
            rig.offsets[i] = BoneOffset {
                t: [0.0, 0.0, 3.5],
                roll: 0.0,
            };
            i
        };
        let baseline = ed
            .source
            .as_ref()
            .unwrap()
            .parsed
            .as_ref()
            .unwrap()
            .model
            .bones[sel]
            .translation[2];

        let out_root = std::env::temp_dir().join("flicker_assetpipeline_commit");
        let _ = std::fs::remove_dir_all(&out_root);
        park(&mut ed, "review");
        ed.commit_to(&out_root);

        let src = ed.source.as_ref().unwrap();
        assert!(src.error.is_none(), "commit reported: {:?}", src.error);
        let written = src
            .committed
            .as_ref()
            .expect("commit recorded where it wrote");
        assert!(
            flicker_content::package::file_exists(written),
            "{} was written",
            written.display()
        );

        // Round-trip through the ENGINE's loader, not a bespoke parse — if the bake drifted from
        // what the runtime accepts, this is where it shows.
        let raw = flicker_content::package::read_text(written).unwrap();
        let json: serde_json::Value = serde_json::from_str(&raw).expect("valid rig json");
        let bones = json["skeleton"]["bones"]
            .as_array()
            .expect("skeleton.bones");
        assert_eq!(
            bones.len(),
            REFERENCE_BONES,
            "the bake synthesized the root"
        );
        assert_eq!(bones[0]["name"], "root", "root is bone 0");

        // The Attach stage's six authored points SHIP (the audited third-step gap: they
        // used to be discarded at export). Each carries its id and canonical parent bone.
        let points = json["attach_points"].as_array().expect("attach_points");
        assert_eq!(
            points.len(),
            ATTACH_POINTS.len(),
            "all six authored points ship"
        );
        for ((id, _, parent), p) in ATTACH_POINTS.iter().zip(points) {
            assert_eq!(p["id"], *id, "point id ships");
            assert_eq!(p["bone"], *parent, "and rides its canonical bone");
        }

        // The authored offset is IN the file: the working model is untouched, the bake carries it.
        assert_eq!(
            ed.source
                .as_ref()
                .unwrap()
                .parsed
                .as_ref()
                .unwrap()
                .model
                .bones[sel]
                .translation[2],
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

    /// THE COMMIT TRANSLATION GUARD (2026-08-20): a character committed from the AS-PROVIDED
    /// editing view (vendor frames live in the bench) must land in staging with CANONICAL joint
    /// orientations — the shared clips play absolute rotations in canonical frames, and commit
    /// is that invariant's output gate. Positions ship as placed; the bench's working model
    /// keeps its vendor frames after commit (the translation belongs to the output, not the view).
    #[test]
    fn committing_an_as_provided_rig_translates_frames_to_canon() {
        let Some(dir) = real_source() else {
            eprintln!("skipping: no content tree");
            return;
        };
        let mut ed = AssetPipeline::shipped();
        ed.pending_class = Some(AssetClass::Skin);
        ed.as_provided = true;
        ed.open(dir);
        assert_eq!(ed.wf.step(), "conform", "open lands on the rig view");

        // A bone's LOCAL frame rotation out of a rig json.
        let local_quat = |json: &serde_json::Value, name: &str| -> glam::Quat {
            let b = json["skeleton"]["bones"]
                .as_array()
                .expect("skeleton.bones")
                .iter()
                .find(|b| b["name"] == name)
                .unwrap_or_else(|| panic!("{name} present"));
            let local = b["local"].as_array().expect("local matrix");
            let mut m = [0.0f32; 16];
            for (i, f) in local.iter().enumerate().take(16) {
                m[i] = f.as_f64().unwrap_or(0.0) as f32;
            }
            let (_, q, _) = glam::Mat4::from_cols_array(&m).to_scale_rotation_translation();
            q
        };
        let model_pelvis = |ed: &AssetPipeline| -> glam::Quat {
            let m = &ed.source.as_ref().unwrap().parsed.as_ref().unwrap().model;
            let i = m
                .bones
                .iter()
                .position(|b| b.name == "pelvis")
                .expect("pelvis present");
            glam::Quat::from_array(m.bones[i].rotation)
        };
        let ref_json: serde_json::Value = serde_json::from_str(
            &flicker_content::package::read_text(&default_reference()).unwrap(),
        )
        .unwrap();
        let canon_pelvis = local_quat(&ref_json, "pelvis");

        // The as-provided working model carries the vendor's pelvis frame, measurably off canon.
        // If this ever reads near-zero the vendor changed conventions and the as-provided view
        // stopped being a distinct state — worth failing loudly over.
        let vendor_pelvis = model_pelvis(&ed);
        assert!(
            vendor_pelvis.angle_between(canon_pelvis).to_degrees() > 10.0,
            "the vendor pelvis frame differs from canon (else as-provided is vacuous)"
        );

        let out_root = std::env::temp_dir().join("flicker_assetpipeline_as_provided_commit");
        let _ = std::fs::remove_dir_all(&out_root);
        park(&mut ed, "review");
        ed.commit_to(&out_root);
        let src = ed.source.as_ref().unwrap();
        assert!(src.error.is_none(), "commit reported: {:?}", src.error);
        let written = src
            .committed
            .as_ref()
            .expect("commit recorded where it wrote");
        let json: serde_json::Value =
            serde_json::from_str(&flicker_content::package::read_text(written).unwrap()).unwrap();
        let committed_pelvis = local_quat(&json, "pelvis");
        assert!(
            committed_pelvis.angle_between(canon_pelvis).to_degrees() < 1.0,
            "the committed pelvis carries the canonical frame, got {:.2}° off",
            committed_pelvis.angle_between(canon_pelvis).to_degrees()
        );
        assert!(
            model_pelvis(&ed).angle_between(vendor_pelvis).to_degrees() < 0.01,
            "the working model keeps the vendor frames after commit"
        );

        let _ = std::fs::remove_dir_all(&out_root);
    }

    /// THE SMOKE-TEST PAGE (Aaron 2026-08-20): the character rail carries a Preview step
    /// right after joint editing — conform → preview → attach → review — and its bake IS
    /// the commit bake (one shared helper), so what the page plays can never drift from
    /// what Export writes. The shared idle must resolve onto the baked bones and pose the
    /// body upright.
    #[test]
    fn the_preview_page_plays_the_commit_bake_under_the_shared_idle() {
        let Some(mut ed) = at_conform() else {
            eprintln!("skipping: no content tree");
            return;
        };
        // The rail: preview sits between the rig view and attach.
        let ids: Vec<&str> = ed.wf_def.steps.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, ["task", "conform", "preview", "attach", "review"]);

        // The page is NAVIGABLE: its surface is on and the footer's Next is live — the
        // dead-page regression (an unconfined grid compositing over the HUD) left the
        // user stranded here with no way forward or back.
        park(&mut ed, "preview");
        {
            let m = ed.hud_model();
            assert!(m.is_on("wf_step_preview"), "the preview surface gates on");
            assert!(m.is_on("next_enabled"), "Next stays live on the preview page");
        }
        // …and Back returns to the rig view (park only walks forward).
        ed.apply_workflow_results(&fired("wf_back"));
        assert_eq!(ed.wf.step(), "conform", "Back returns to joint editing");

        // Author a distinctive joint move so the preview must carry it.
        {
            let src = ed.source.as_mut().unwrap();
            let i = src.parsed.as_ref().unwrap().bone_index("head").unwrap();
            let rig = src.rig.as_mut().unwrap();
            rig.offsets[i] = BoneOffset {
                t: [0.0, 0.0, 3.5],
                roll: 0.0,
            };
        }

        let (_rig_file, bones, clip) = ed.bake_preview_parts().expect("the preview bakes");
        assert!(
            !clip.tracks.is_empty(),
            "the shared idle resolves onto the baked bones"
        );

        // The preview IS the commit: the written file carries the same skeleton bone-for-bone.
        let out_root = std::env::temp_dir().join("flicker_assetpipeline_bake_preview");
        let _ = std::fs::remove_dir_all(&out_root);
        park(&mut ed, "review");
        ed.commit_to(&out_root);
        let src = ed.source.as_ref().unwrap();
        assert!(src.error.is_none(), "commit reported: {:?}", src.error);
        let written = src
            .committed
            .as_ref()
            .expect("commit recorded where it wrote");
        let json: serde_json::Value =
            serde_json::from_str(&flicker_content::package::read_text(written).unwrap()).unwrap();
        let wb = json["skeleton"]["bones"].as_array().expect("skeleton.bones");
        assert_eq!(wb.len(), bones.len(), "same skeleton size as the preview");
        for (i, b) in bones.iter().enumerate() {
            assert_eq!(wb[i]["name"], b.name, "bone {i} matches the preview");
            let l = wb[i]["local"].as_array().expect("local");
            let stored: Vec<f32> = l.iter().map(|v| v.as_f64().unwrap() as f32).collect();
            let ours = b.local.to_cols_array();
            for k in 0..16 {
                assert!(
                    (stored[k] - ours[k]).abs() < 1e-2,
                    "bone {} local[{k}] drifted: {} vs {}",
                    b.name,
                    stored[k],
                    ours[k]
                );
            }
        }

        // The smoke test's own smoke test: mid-idle the baked body poses UPRIGHT
        // (Z-tallest in source space) — a Katanami-class contortion would fail this.
        let locals = sample_local_poses(&bones, &clip, 100, true);
        let globals = global_transforms(&bones, &locals);
        let (mut min, mut max) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
        for g in &globals {
            let p = g.w_axis.truncate();
            min = min.min(p);
            max = max.max(p);
        }
        let d = max - min;
        assert!(
            d.z > d.x && d.z > d.y,
            "the animated preview stands tall along Z, got extents {d:?}"
        );

        let _ = std::fs::remove_dir_all(&out_root);
    }

    /// The bone map's colours resolve to REAL rgba through the SCENE FILE's folded
    /// style blocks — a typo'd or deleted `assetpipeline.map.*` key would otherwise
    /// show as default ink and read as fine. The pair script's wash + chip paths ride
    /// the same file, so they are pinned here too.
    #[test]
    fn bone_map_colours_resolve_against_the_scene_styles() {
        let def = SceneDef::parse("assetpipeline", AP_SCENE).expect("scene file parses");
        let styles = flicker::ui::load_shared_styles(def.styles.as_ref());
        for state in [MapState::Ok, MapState::Review, MapState::Auto] {
            let mut node = &styles;
            for part in state.color().split('.') {
                node = &node[part];
            }
            let rgba = node.as_array().unwrap_or_else(|| {
                panic!(
                    "{} did not resolve to an rgba array: {node:?}",
                    state.color()
                )
            });
            assert_eq!(rgba.len(), 4, "{} is rgba", state.color());
        }
        // The Review stage's failure colour rides the same block.
        assert!(styles["assetpipeline"]["map"]["fail"].is_array());
        // The pair script's derived paths: the rail-chip trio + both bank washes
        // must be real blocks, or a derived name would fail to nothing.
        for chip in ["active", "visited", "todo"] {
            assert!(
                styles["workflow"]["chip"][chip].is_object(),
                "workflow.chip.{chip} is authored"
            );
        }
        assert!(styles["assetpipeline"]["rowsel"].is_object());
        assert!(styles["assetpipeline"]["rowsel_off"].is_object());
    }

    /// The right-drag pan slides the LOOK-AT POINT across the view plane so the content tracks the
    /// cursor. Pure math — no GPU, no content.
    #[test]
    fn right_drag_pan_tracks_the_cursor_without_rotating_the_view() {
        // Look down +X at the origin (yaw 0, pitch 0): screen-right is +Y, screen-up is +Z.
        let mut o = Orbit {
            yaw: 0.0,
            pitch: 0.0,
            dist_scale: 2.4,
            zoom: 1.0,
            pan: Vec3::ZERO,
        };
        let (radius, vh) = (100.0_f32, 800.0_f32);
        let before = o.camera(radius);
        let persp = o.camera(radius);

        // Cursor RIGHT: the content follows it, so the target slides the OTHER way along
        // screen-right (+Y). Getting this backwards is the classic inverted-pan bug.
        o.pan_by_view(Vec2::new(10.0, 0.0), &persp, vh);
        assert!(
            o.pan.y < 0.0,
            "dragging right slides the target left, got {:?}",
            o.pan
        );
        assert!(
            o.pan.x.abs() < 1e-4,
            "and stays in the view plane, got {:?}",
            o.pan
        );

        // Cursor DOWN (screen Y grows downward): the target rises, so the asset slides down.
        o.pan = Vec3::ZERO;
        o.pan_by_view(Vec2::new(0.0, 10.0), &persp, vh);
        assert!(
            o.pan.z > 0.0,
            "dragging down raises the target, got {:?}",
            o.pan
        );

        // A pan TRANSLATES the camera rigidly — eye and target move together. If only the target
        // moved this would silently become an orbit.
        let after = o.camera(radius);
        let d_eye = after.position - before.position;
        let d_tgt = after.target - before.target;
        assert!(
            (d_eye - d_tgt).length() < 1e-4,
            "eye and target must move together"
        );
        let dir_before = (before.position - before.target).normalize();
        let dir_after = (after.position - after.target).normalize();
        assert!(
            (dir_before - dir_after).length() < 1e-5,
            "a pan must not rotate the view"
        );

        // 1:1 at the look-at depth: dragging a full viewport height moves exactly the height
        // visible there, so the pan neither crawls on a large asset nor bolts on a small one.
        o.pan = Vec3::ZERO;
        o.pan_by_view(Vec2::new(0.0, vh), &persp, vh);
        let visible_h = 2.0 * o.dist(radius) * (ORBIT_FOV_Y * 0.5).tan();
        assert!(
            (o.pan.length() - visible_h).abs() < 1e-2,
            "expected a 1:1 pan of {visible_h}, got {}",
            o.pan.length()
        );

        // Scale follows the framing: the same drag on a 10× larger asset pans 10× further.
        let mut big = Orbit {
            yaw: 0.0,
            pitch: 0.0,
            dist_scale: 2.4,
            zoom: 1.0,
            pan: Vec3::ZERO,
        };
        let big_cam = big.camera(radius * 10.0);
        big.pan_by_view(Vec2::new(0.0, vh), &big_cam, vh);
        assert!(
            (big.pan.length() / o.pan.length() - 10.0).abs() < 1e-3,
            "pan scales with framing"
        );

        // An ORTHOGRAPHIC quad pans in ITS OWN plane, not the perspective one — that is the whole
        // reason `pan_by_view` takes a camera. TOP looks down +Z with +Y up the panel (the Z-up
        // `EDITOR_QUADS`), so its screen-right is +X.
        let top = Camera {
            position: Vec3::new(0.0, 0.0, 400.0),
            target: Vec3::ZERO,
            up: Vec3::Y,
            fov_y_radians: ORBIT_FOV_Y,
            near: 0.01,
            far: 1200.0,
            ortho_height: Some(220.0),
        };
        let mut t = Orbit {
            yaw: 0.0,
            pitch: 0.0,
            dist_scale: 2.4,
            zoom: 1.0,
            pan: Vec3::ZERO,
        };
        t.pan_by_view(Vec2::new(10.0, 0.0), &top, vh);
        assert!(
            t.pan.x < 0.0,
            "dragging right in TOP slides the target along -X, got {:?}",
            t.pan
        );
        assert!(
            t.pan.z.abs() < 1e-4,
            "and never along the view's OWN axis, got {:?}",
            t.pan
        );
        // An ortho camera states its visible height, so the scale is that height over the viewport
        // — using the perspective formula here would pan at the wrong rate entirely.
        t.pan = Vec3::ZERO;
        t.pan_by_view(Vec2::new(0.0, vh), &top, vh);
        assert!(
            (t.pan.length() - 220.0).abs() < 1e-2,
            "1:1 against the ortho height"
        );
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
        assert!(
            o.ortho_radius(radius) < r0,
            "and tightens the ortho framing with it"
        );
        let (fd, fr) = (o.dist(radius) / d0, o.ortho_radius(radius) / r0);
        assert!(
            (fd - fr).abs() < 1e-5,
            "perspective and ortho must zoom by the SAME factor"
        );

        o.zoom_by(-1.0); // and back out
        assert!(
            (o.dist(radius) - d0).abs() / d0 < 0.02,
            "a notch out ~undoes a notch in"
        );

        // Clamped at both ends: the wheel can never invert the camera or run away to nothing.
        for _ in 0..500 {
            o.zoom_by(1.0);
        }
        assert!(
            o.zoom >= 0.05 && o.dist(radius) > 0.0,
            "zoom floors, never inverts"
        );
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
        assert!(
            base.verts.len() <= BASE_MESH_BUDGET,
            "and it fits the upload budget"
        );
        // A well-formed triangle list that indexes only real vertices — a bad one would fault the
        // draw rather than merely look wrong.
        assert!(
            !base.indices.is_empty() && base.indices.len() % 3 == 0,
            "a triangle list"
        );
        let n = base.verts.len() as u32;
        assert!(
            base.indices.iter().all(|i| *i < n),
            "every index is inside the vertex list"
        );

        // Framing now comes from the MESH when there is one, so the stage floor is the SOLE of the
        // foot rather than the lowest JOINT (the ankle sits well above the sole — `ANKLE_FRACTION`).
        // Getting this wrong floats the body above its own grid.
        assert!(base.floor < 0.0, "the recentred floor is below the origin");
        let lowest_vert = base
            .verts
            .iter()
            .map(|v| v.position[2])
            .fold(f32::MAX, f32::min);
        let lowest_joint = base
            .globals
            .iter()
            .map(|g| g.w_axis.z)
            .fold(f32::MAX, f32::min);
        assert!(
            lowest_vert <= lowest_joint + 1e-3,
            "the mesh must reach at or below the lowest joint (sole {lowest_vert}, joint {lowest_joint})"
        );
    }

    // ── the canonical-pattern gates (rule E5AFBBAB) ────────────────────────────────
    // Three assertions that hold Clayworks on the Quartermaster shape. They exist
    // because the regression they guard is INVISIBLE: a hand-composed surface looks
    // identical on screen to a configured one, so only a test can tell them apart.

    /// EVERY DECLARED INTENT REACHES THE DISPATCHER. A result name the scene never
    /// acts on is dead hardware: the button is bound, the signal fires, nothing
    /// happens. `pause_open` is read in `update`'s pause arm; the rest are results
    /// the frame's dispatch consumes.
    #[test]
    fn every_declared_intent_reaches_the_dispatcher() {
        load_shipped_strings();
        let mut ed = AssetPipeline::shipped();

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
        assert_eq!(
            ed.wf.step(),
            "task",
            "wf_back — the declared Cancel/TabPrev result — steps the rail"
        );
    }

    /// The rail SKIPS what a class does not use — BY DEFINITION now: the prop workflow
    /// simply has no `attach` step (Attach defines the sockets a BODY OFFERS; a prop mounts
    /// TO one, authored at Conform), so neither Next nor Back can ever stop there, and its
    /// rail is Workflow · Mount · Review. No content needed: this is the dispatch itself.
    #[test]
    fn a_non_character_workflow_has_no_attach_step() {
        load_shipped_strings();
        let mut ed = AssetPipeline::shipped();
        ed.dispatch_workflow(Some(AssetClass::Prop));
        assert_eq!(ed.wf_def.steps.len(), 3, "task → conform → review");
        assert!(
            ed.wf_def.steps.iter().all(|s| s.id != "attach"),
            "the prop definition carries no character-only Attach"
        );

        // Back from Review lands on Mount — there is no attach between to strand the user on.
        park(&mut ed, "review");
        ed.apply_workflow_results(&fired("wf_back"));
        assert_eq!(
            ed.wf.step(),
            "conform",
            "Back hops straight to the rig (Mount) view"
        );

        // The rail: three chips, the conform chip named for its definition's role.
        let m = ed.hud_model();
        assert_eq!(
            m.text("wf_conform_title"),
            Some("Mount"),
            "the prop definition names its conform stop"
        );
        assert!(!m.is_on("wf_attach_show"), "no character-only Attach chip");
        assert!(m.is_on("wf_task_show") && m.is_on("wf_conform_show") && m.is_on("wf_review_show"));
        assert_eq!(
            m.number("wf_step_n"),
            Some(3.0),
            "the footer counts three stops"
        );
    }

    /// The rail chips ride the workflow RUNTIME's published binds: the current step
    /// publishes state `active`, walked-past steps `visited`, later ones `todo` (the
    /// pair script maps those onto the `workflow.chip.*` styles), and every chip title
    /// arrives PRE-LOCALIZED through the stringtable — while the step's surface key
    /// gates its content subtree, exclusively.
    #[test]
    fn rail_chips_ride_the_workflow_runtime_binds() {
        load_shipped_strings();
        let mut ed = AssetPipeline::shipped();
        park(&mut ed, "conform"); // the rig-edit view, reached right after choosing a workflow
        let m = ed.hud_model();
        // Load / Analyze / Classify are gone (the workflow selector + inline parse/conform
        // replaced them), so the character rail reads Workflow · Rig · Attach · Review.
        assert_eq!(m.text("wf_task_title"), Some("Workflow"));
        assert_eq!(
            m.text("wf_conform_title"),
            Some("Rig"),
            "the character definition's conform stop"
        );
        assert_eq!(m.text("wf_attach_title"), Some("Attach"));
        assert_eq!(m.text("wf_review_title"), Some("Review"));
        // The current step publishes its state; behind reads visited, ahead todo —
        // the pair script turns each into its `workflow.chip.*` path (gated below).
        assert_eq!(m.text("wf_conform_state"), Some("active"));
        assert_eq!(m.text("wf_task_state"), Some("visited"));
        assert_eq!(m.text("wf_attach_state"), Some("todo"));
        // All five chips of the character definition are shown, and the footer counts them.
        for id in ["task", "conform", "preview", "attach", "review"] {
            assert!(m.is_on(&format!("wf_{id}_show")), "{id} chip shown");
        }
        assert_eq!(m.number("wf_step_n"), Some(5.0));
        // The step SURFACES (namespaced `wf_step_<id>`) gate the content subtrees,
        // exclusively on the current step.
        assert!(
            m.is_on("wf_step_conform"),
            "the current step's surface is on"
        );
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
        let mut ed = AssetPipeline::shipped();
        assert!(
            !ed.wf.can_next(&ed.wf_doc()),
            "no source open — `conform` needs one"
        );
        ed.apply_workflow_results(&fired("wf_next"));
        assert_eq!(ed.wf.step(), "task", "the refused advance holds the page");
        park(&mut ed, "review");
        assert!(!ed.wf.can_next(&ed.wf_doc()), "Review is the last step");
        assert!(
            ed.hud_model().is_on("next_enabled"),
            "…whose button stays live as Restart"
        );
    }

    /// "Restart" on the last page is SCENE policy over the runtime: it drops the finished
    /// asset and rebuilds the DEFAULT (character) workflow back on Task — and the shipped
    /// scene file's `params.workflows` genuinely ships all three dispatch targets.
    #[test]
    fn restart_on_review_returns_to_task_with_the_default_definition() {
        let mut ed = AssetPipeline::shipped();
        assert!(
            ed.workflows.contains_key(WF_CHARACTER)
                && ed.workflows.contains_key(WF_PROP)
                && ed.workflows.contains_key(WF_ANIMATION),
            "the scene file ships all three dispatch targets"
        );
        ed.dispatch_workflow(Some(AssetClass::Prop));
        ed.pending_class = Some(AssetClass::Prop);
        park(&mut ed, "review");
        ed.apply_workflow_results(&fired("wf_next"));
        assert_eq!(ed.wf.step(), "task", "the wizard is back on the entry page");
        assert!(
            ed.source.is_none() && ed.pending_class.is_none(),
            "the per-asset state dropped"
        );
        assert_eq!(
            ed.wf_def.steps.len(),
            5,
            "…and the default (character) definition is restored"
        );
    }

    /// The real Motifect BVH library — the animation workflow's genuine input. Skips
    /// when the content tree (the clips or the reference rig) is absent, like every
    /// other real-data test here.
    fn real_bvh_source() -> Option<PathBuf> {
        let dir = PathBuf::from(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../content/source/characters/Motifect/Motifect_combat_complete_v1_0/BVH"
        ));
        let reference = default_reference();
        let have_ref = reference.exists() || reference.with_extension("json.gz").exists();
        (dir.exists() && have_ref).then_some(dir)
    }

    /// ANIMATION IMPORT, end to end on the real library: the Task card's class routes to
    /// `import_animation`, the folder's BVH files fill the SAME picker meshes use, the
    /// stage runner retargets the active clip IN MEMORY, the `clip` doc gate opens, and
    /// Commit honours the side-by-side pick — writing exactly the chosen variants.
    #[test]
    fn an_animation_walks_retarget_preview_and_commits_the_picked_variants() {
        let Some(dir) = real_bvh_source() else {
            eprintln!("skipping: no content tree");
            return;
        };
        let mut ed = AssetPipeline::shipped();
        ed.pending_class = Some(AssetClass::Animation);
        ed.open(dir);
        assert_eq!(
            ed.wf.step(),
            "conform",
            "an animation lands on its Clip page"
        );
        {
            let src = ed.source.as_ref().unwrap();
            assert!(
                src.candidates.len() > 1,
                "the combat library offers a clip choice"
            );
            assert!(
                src.candidates
                    .iter()
                    .all(|p| p.extension().is_some_and(|e| e == "bvh")),
                "candidates are the folder's BVH clips"
            );
            assert!(
                src.error.is_none(),
                "no invented 'no riggable mesh': {:?}",
                src.error
            );
            assert!(src.clip.is_none(), "retarget waits for the stage runner");
        }

        // The conform-step runner (`update` calls this beside analyze/conform).
        ed.prepare_clip();
        let doc = ed.wf_doc();
        {
            let src = ed.source.as_ref().unwrap();
            let cp = src
                .clip
                .as_ref()
                .unwrap_or_else(|| panic!("retarget: {:?}", src.error));
            assert!(cp.duration > 0, "a real clip has length");
            assert!(
                !cp.ip.tracks.is_empty() && !cp.rm.tracks.is_empty(),
                "both variants resolve"
            );
            assert_eq!(cp.bones.len(), cp.parents.len());
            assert!(
                cp.rm_radius >= cp.radius,
                "the RootMotion frame is never tighter than rest"
            );
        }
        assert!(doc.is_on("clip"), "the doc gate opens on the built preview");
        assert!(ed.wf.can_next(&doc), "Next reaches Review");

        // Nothing picked → an honest refusal, no files.
        let scratch = std::env::temp_dir().join("flicker_assetpipeline_clip_commit");
        let _ = std::fs::remove_dir_all(&scratch);
        ed.variant_ip = false;
        ed.variant_rm = false;
        ed.commit_to(&scratch);
        {
            let src = ed.source.as_ref().unwrap();
            assert!(
                src.error
                    .as_deref()
                    .unwrap_or("")
                    .contains("at least one variant"),
                "an empty pick refuses: {:?}",
                src.error
            );
            assert!(src.committed.is_none());
        }

        // Root Motion alone → exactly that variant lands, In-Place does not.
        ed.variant_rm = true;
        ed.commit_to(&scratch);
        let src = ed.source.as_ref().unwrap();
        assert!(
            src.error.is_none(),
            "the picked commit succeeds: {:?}",
            src.error
        );
        let set = scratch.join(src.asset_name());
        assert!(
            set.join("RootMotion").is_dir(),
            "the picked variant is written"
        );
        assert!(!set.join("In-Place").exists(), "the unpicked one is NOT");
        let out = src.committed.clone().expect("a committed path is recorded");
        let text = flicker_content::package::read_text(&out).expect("the emitted clip reads back");
        assert!(
            text.contains("\"retarget\":true"),
            "a clip ships retarget:true"
        );
        let _ = std::fs::remove_dir_all(&scratch);
    }

    /// THE SILENT-COMMIT REGRESSION (QA 2026-08-03: "doesn't always end up producing an
    /// object in the staging folder"). A prop folder whose mesh never parsed must (a) be
    /// STOPPED at the Mount page — `fit` no longer publishes without a parse — and (b)
    /// refuse Export OUT LOUD if commit is ever reached, instead of returning silently
    /// with no file and no message.
    #[test]
    fn an_unparsed_prop_cannot_reach_review_and_export_refuses_loudly() {
        let scratch = std::env::temp_dir().join("flicker_assetpipeline_unparsed_prop");
        let _ = std::fs::remove_dir_all(&scratch);
        std::fs::create_dir_all(&scratch).unwrap();
        let mut ed = AssetPipeline::shipped();
        ed.pending_class = Some(AssetClass::Prop);
        ed.pending_prop = Some(PropKind::Environment);
        ed.open(scratch.clone());
        assert_eq!(
            ed.wf.step(),
            "conform",
            "the empty folder still lands on the Mount page"
        );

        let doc = ed.wf_doc();
        assert!(
            !doc.is_on("fit"),
            "no parse → no mount binding → no fit key"
        );
        assert!(
            !ed.wf.can_next(&doc),
            "Review is gated off — nothing walks to a dead Export"
        );

        let out = std::env::temp_dir().join("flicker_assetpipeline_unparsed_prop_out");
        let _ = std::fs::remove_dir_all(&out);
        ed.commit_to(&out);
        let src = ed.source.as_ref().unwrap();
        assert!(
            src.error
                .as_deref()
                .unwrap_or("")
                .contains("nothing to commit"),
            "the refusal is SAID, not silent: {:?}",
            src.error
        );
        assert!(
            src.committed.is_none() && !out.exists(),
            "and nothing was written"
        );
        let _ = std::fs::remove_dir_all(&scratch);
    }

    /// Every workflow's commit lands in a STAGING tier, routed by what the asset IS:
    /// clips → the shared retarget library, environment props → props/, characters and
    /// worn things → characters/. The Quartermaster's promote pass is the only door
    /// into package/ — the one tree the engine loads content from.
    #[test]
    fn commit_roots_route_by_class_and_all_land_in_staging() {
        let case = |class: Option<AssetClass>, prop: Option<PropKind>, suffix: &str| {
            let scratch = std::env::temp_dir().join(format!(
                "flicker_assetpipeline_root_{}",
                suffix.replace('/', "_")
            ));
            let _ = std::fs::remove_dir_all(&scratch);
            std::fs::create_dir_all(&scratch).unwrap();
            let mut ed = AssetPipeline::shipped();
            ed.pending_class = class;
            ed.pending_prop = prop;
            ed.open(scratch.clone());
            let root = ed.commit_root();
            assert!(
                root.ends_with(suffix),
                "{class:?}/{prop:?} routes to …/{suffix}, got {}",
                root.display()
            );
            assert!(
                root.strip_prefix(flicker_content::roots().staging())
                    .is_ok(),
                "every commit root is inside staging/: {}",
                root.display()
            );
            let _ = std::fs::remove_dir_all(&scratch);
        };
        case(Some(AssetClass::Animation), None, "staging/retarget/clips");
        case(
            Some(AssetClass::Prop),
            Some(PropKind::Environment),
            "staging/props",
        );
        case(
            Some(AssetClass::Prop),
            Some(PropKind::Clothing),
            "staging/characters",
        );
        case(Some(AssetClass::Skin), None, "staging/characters");
    }

    /// An animation folder WITHOUT clips reports the real absence — never the mesh
    /// path's "no riggable mesh", which would send the user hunting the wrong problem.
    #[test]
    fn an_animation_folder_without_bvh_reports_the_real_absence() {
        let scratch = std::env::temp_dir().join("flicker_assetpipeline_no_bvh");
        let _ = std::fs::remove_dir_all(&scratch);
        std::fs::create_dir_all(&scratch).unwrap();
        let mut ed = AssetPipeline::shipped();
        ed.pending_class = Some(AssetClass::Animation);
        ed.open(scratch.clone());
        let src = ed.source.as_ref().unwrap();
        assert!(
            src.error.as_deref().unwrap_or("").contains("No BVH"),
            "the error names the real absence: {:?}",
            src.error
        );
        let _ = std::fs::remove_dir_all(&scratch);
    }

    /// A conform edit arms the Back DISCARD GUARD: Back holds the page and raises the
    /// `wf_discard` surface, "keep editing" stays put, "discard" steps back.
    #[test]
    fn dirty_back_raises_the_discard_dialog() {
        load_shipped_strings();
        let mut ed = AssetPipeline::shipped();
        park(&mut ed, "conform");
        ed.wf.set_dirty(true); // as a slider/gizmo edit would through `update`
        ed.apply_workflow_results(&fired("wf_back"));
        assert_eq!(ed.wf.step(), "conform", "held until the dialog resolves");
        let m = ed.hud_model();
        assert!(m.is_on("wf_discard"), "the confirm dialog surface is up");

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
        let ed = AssetPipeline::shipped();
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
        assert_eq!(
            centre,
            Vec3::new(0.0, 0.0, 13.5),
            "midway between the root and the tip"
        );
        assert_eq!(radius, 3.5, "half the 10 → 17 span");
        // The floor is the feet plane AFTER the same `-centre` shift the viewport draws through,
        // so it is negative and lands exactly on the lowest bone — draw the stage grid at the
        // asset's soles, not at the origin (which recentring puts at its waist).
        assert_eq!(floor, -3.5, "lowest extent (z=10) recentred about 13.5");
        assert!(floor < 0.0, "a recentred floor is always below the origin");
    }

    // ── the scene-pair gates (the migrated shape) ──────────────────────────────

    /// The shipped scene file IS the bench: it parses, names this behaviour, authors
    /// a tree, and its `params.workflows` carries all three dispatch targets — the
    /// same file the manifest hands the factory.
    #[test]
    fn the_shipped_scene_file_authors_the_bench() {
        let def = SceneDef::parse("assetpipeline", AP_SCENE).expect("scene file parses");
        assert_eq!(def.behaviour, "assetpipeline");
        assert!(def.tree.is_some(), "the scene file carries the HUD tree");
        let defs = workflows_from_params(&def.params);
        for name in [WF_CHARACTER, WF_PROP, WF_ANIMATION] {
            assert!(defs.contains_key(name), "params.workflows ships `{name}`");
        }
    }

    /// THE PAIR-SCRIPT REGRESSION GATE: build the bench exactly as the resolver does
    /// (real def, real assetpipeline.lua) and run the REAL model path — the published
    /// step states must come back as the DERIVED `workflow.chip.*` style paths the
    /// chips' `style_bind`s name, and every bank row must carry a wash path. A
    /// derive() that throws leaves the keys absent.
    #[test]
    fn the_pair_script_derives_the_chip_styles_and_washes() {
        load_shipped_strings();
        let mut ed = AssetPipeline::shipped();
        assert!(ed.script.is_some(), "assetpipeline.lua loads (the pair script)");
        let m = ed.model();
        assert_eq!(
            m.text("wf_task_style"),
            Some("workflow.chip.active"),
            "the entry step's chip lights via the derived path"
        );
        for id in ["conform", "attach", "review"] {
            assert_eq!(
                m.text(&format!("wf_{id}_style")),
                Some("workflow.chip.todo"),
                "`{id}` is ahead of the entry step"
            );
        }
        for bank in ["pick", "bone", "sock", "att"] {
            for i in 0..BONE_ROWS {
                let key = format!("{bank}_{i}_sty");
                let path = m.text(&key).unwrap_or_default();
                assert!(
                    path == "assetpipeline.rowsel" || path == "assetpipeline.rowsel_off",
                    "{key} carries a wash path, got {path:?}"
                );
            }
        }
        // The selected slot of an un-windowed bank wears the wash.
        assert_eq!(m.text("att_0_sty"), Some("assetpipeline.rowsel"));
        assert_eq!(m.text("att_1_sty"), Some("assetpipeline.rowsel_off"));
    }

    /// Walk the REAL tree with the REAL derived model and gate the authored data:
    /// known kinds only, no raw display literals in the tree, no raw display copy
    /// published from Rust into the Model — and the walk actually draws the bench.
    #[test]
    fn hud_tree_walks_with_model() {
        load_shipped_strings();
        let def = SceneDef::parse("assetpipeline", AP_SCENE).expect("scene file parses");
        let tree = def.tree.clone().expect("scene defines a tree");
        let styles = flicker::ui::load_shared_styles(def.styles.as_ref());

        // Vocabulary gate: an unknown kind renders NOTHING, so a name left behind
        // by a rename would be invisible until someone opened the window.
        assert!(
            flicker::ui::unknown_kinds(&tree).is_empty(),
            "assetpipeline.scene.json names unknown kinds: {:?}",
            flicker::ui::unknown_kinds(&tree)
        );
        // The strings gate (S10): every display literal is a `$token`.
        assert!(
            flicker::ui::raw_display_literals(&tree).is_empty(),
            "assetpipeline.scene.json ships raw display literals: {:?}",
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

        let mut ed = AssetPipeline::shipped();
        let m = ed.model();
        let snap = UiInput {
            mouse: Vec2::new(-1.0, -1.0),
            clicked: false,
            down: false,
            screen: Vec2::new(1920.0, 1080.0),
            typed: String::new(),
            backspace: false,
            wheel: 0.0,
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
        assert!(has_text("Clayworks Bench"), "the header title renders");
        assert!(has_text("CHOOSE A WORKFLOW"), "the Task page renders");
        assert!(has_text("Workflow"), "the entry rail chip renders");
        assert!(has_text("NEXT"), "the footer's forward button renders");
    }

    /// The declared pause intent through the scene's REAL 2-layer chain — the
    /// authored root's `on_menu` reaches the walker, which consumes the Menu press
    /// and fires the name the ONE dispatch maps onto the pause push (the re-pointed
    /// half of the retired route.rs tests).
    #[test]
    fn the_declared_pause_intent_fires_through_the_authored_tree() {
        use flicker_input_core::{EventKind, InputContext};

        let def = SceneDef::parse("assetpipeline", AP_SCENE).expect("scene file parses");
        let tree = def.tree.expect("scene defines a tree");
        let intents = UiIntents::of(&tree);

        let raw = InputState::new();
        let events = [InputEvent::new(
            ActionSignal::Menu,
            EventKind::Press,
            InputContext::World,
            &raw,
        )];
        let mut ui = UiState::new();
        let mut walker = WalkerHandler::hud(&mut ui, false).with_intents(&intents);
        let mut editor = EditorLayer::default();
        let mut rc = RouteCtx::new();
        let report = {
            let mut chain: [&mut dyn InputHandler; 2] = [&mut walker, &mut editor];
            Router::dispatch(&events, &mut chain, &mut rc)
        };
        assert!(
            report.consumed_by(0, ActionSignal::Menu),
            "the walker layer consumed the declared Menu"
        );
        assert_eq!(
            walker.take_fired(),
            vec!["pause_open".to_string()],
            "the fired name is the pause-open edge the dispatch maps"
        );
    }

    /// The editor layer owns the camera signals ONLY while the viewport pane is
    /// entered — everywhere else they pass to whatever sits below (the GlobeWorld
    /// contract, replicated).
    #[test]
    fn the_editor_layer_gates_camera_signals_on_pane_entry() {
        use flicker_input_core::{EventKind, InputContext};

        let raw = InputState::new();
        let ev = |signal| InputEvent::new(signal, EventKind::Press, InputContext::World, &raw);
        let mut rc = RouteCtx::new();
        let mut editor = EditorLayer::default();
        assert_eq!(
            editor.handle(&ev(ActionSignal::LookLeft), &mut rc),
            Flow::Pass,
            "not entered → the look signal passes"
        );
        editor.owns_camera = true;
        assert_eq!(
            editor.handle(&ev(ActionSignal::LookLeft), &mut rc),
            Flow::Consumed,
            "entered → the look signal stops here"
        );
        assert_eq!(
            editor.handle(&ev(ActionSignal::PrimaryAction), &mut rc),
            Flow::Pass,
            "non-camera signals always pass"
        );
    }
}
