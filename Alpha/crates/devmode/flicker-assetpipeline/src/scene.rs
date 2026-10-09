//! **The bench's BEHAVIOUR** — the thin scene that plays the authored tree.
//!
//! The canonical shape (Populous is the reference): the tree is DATA
//! (`assetpipeline.scene.json`), `arrange()` in `assetpipeline.lua` lights the selected
//! workflow's rail and step slice, and this file does exactly four things a frame —
//! publish the Model from the [`Document`], walk the tree, dispatch the pump's events
//! through the walker, and fold the ONE results drain into the document's services.
//! It reads no device, owns no focus system, builds no structure, and formats every
//! readout before it reaches a node.

use std::collections::HashMap;
use std::time::Duration;

use flicker::render::{FrameGraph, MeshDrawOptions, Renderer, TextureHandle};
use flicker::scene::{Scene, SceneInput, Transition};
use flicker::script::{HudCommand, ScriptHost, UiNode, Value, ValueMap};
use flicker::ui::{
    instantiate_rows, render_hud, run_ui, strings, Row, SceneDef, UiInput, UiIntents, UiState,
    WalkerHandler,
};
use flicker_content::{AssetClass, PropKind, StanceSource};
use flicker_globe::GlobeWorld;
use flicker_input_core::{AbstractControls, GamepadConfig, InputContext, InputMap, InputState};
use flicker_input_router::{InputHandler, Router};
use flicker_modelview::{ModelView, ViewContext};
use flicker_rigview::gadget::modes_from_names;
use flicker_rigview::{Draw, GadgetStyle, Projection};

use flicker_skeletal::format::{ArmKind, LegKind, TailKind};

use crate::compose::{BoneFilter, CentreStyle};
use crate::services::Side;

/// A part framed alone (FRAME on a single joint) fills this fraction of the subject's radius.
const FRAME_MIN_FRAC: f32 = 0.08;

/// The side the quad view isolates: the SIDE panel's flip is the one current side — LEFT shows
/// the left limbs, RIGHT the right — and FRONT/BACK and TOP/BOTTOM follow it (they have no
/// near side of their own).
fn isolated_side(side_panel_flipped: bool) -> Side {
    if side_panel_flipped {
        Side::Right
    } else {
        Side::Left
    }
}

/// What rig panel `panel` (0 = perspective, then top, side, front) draws: SOLO — the bench's
/// own Rig control — wins wherever a joint is selected; else the panel's OWN LIMB checkbox
/// isolates `side`; the perspective picker follows whenever ANY panel isolates, dimming rather
/// than hiding. `None` draws everything.
///
/// `limb` is the three orthographic panels' checkboxes in `RIG_SLOTS[1..]` order, read back
/// off each panel (`ModelView::isolate`) — the chrome is the PANEL's now, so this is
/// what the bench knows about it.
fn panel_filter(
    solo: bool,
    limb: [bool; 3],
    panel: usize,
    side: Side,
    sel: Option<usize>,
) -> Option<BoneFilter> {
    if solo {
        if let Some(s) = sel {
            return Some(BoneFilter::Subtree(s));
        }
    }
    let on = match panel {
        0 => limb.iter().any(|&on| on),
        i => limb.get(i - 1).copied().unwrap_or(false),
    };
    on.then_some(BoneFilter::Limb(side))
}

/// The `scene_<name>` props the tree authors on the surface node `id`, keyed by `<name>` —
/// the same map the walker reserves into `SurfaceSlot::params` (rule E5AFBBAB: the panel's
/// projection is AUTHORED data, never a compiled-in list). Read once at construction,
/// because the scene a view surface names is built once and seated every frame.
fn scene_params(tree: &UiNode, id: &str) -> ValueMap {
    fn find<'a>(node: &'a UiNode, id: &str) -> Option<&'a UiNode> {
        if node.id == id {
            return Some(node);
        }
        node.children.iter().find_map(|c| find(c, id))
    }
    let node = find(tree, id)
        .unwrap_or_else(|| panic!("the tree authors no `{id}` surface for the bench to seat"));
    // The walker carries a scene NAME and resolves nothing; a bad name fails loud where the
    // HOST looks it up, which is here. This bench builds `model_view` panels and only those.
    let scene = match node.props.get("scene") {
        Some(Value::Text(name)) => name.as_str(),
        _ => "",
    };
    assert_eq!(
        scene,
        ui::VIEW_SCENE,
        "`{id}` authors the sub scene `{scene}` — this bench builds `{}` panels",
        ui::VIEW_SCENE
    );
    let mut params = ValueMap::new();
    for (key, value) in &node.props {
        if let Some(name) = key.strip_prefix("scene_") {
            params.set(name, value.clone());
        }
    }
    params
}
use flicker_shell::{ModalParams, PauseScene, SharedModal, SubScene, Theme};
use glam::{Mat4, Vec3};

use crate::compose::{self, Show};
use crate::gizmo::{Gizmo, GizmoUi, PanelFacts};
use crate::meshes::ViewMeshes;
use crate::services::MarkerStep;
use crate::services::{
    self, class_label, BoneOffset, Document, MapState, RegionEdit, WF_ANIMATION, WF_CHARACTER,
    WF_CREATURE, WF_PROP,
};
use crate::ui::{self, Step, Workflow};

/// The Clayworks bench.
pub struct Clayworks {
    /// The document + its services (scan / analyze / conform / bake / commit).
    doc: Document,
    /// The authored tree, walked every frame (rows expanded per frame from the document).
    tree: UiNode,
    script: ScriptHost,
    ui_styles: serde_json::Value,
    ui_state: UiState,
    ui_intents: UiIntents,
    hud_commands: Vec<HudCommand>,
    theme: Option<Theme>,
    textures: Vec<TextureHandle>,
    /// The selection `arrange()` reads: the open workflow and the rail's step index.
    wf: Workflow,
    tab: usize,
    /// The unsaved-work prompt, armed for THIS frame's `update` to push as the shared
    /// `choice_dialog` modal. The dispatcher cannot push a scene itself (it returns
    /// nothing), so it arms and `update` — which owns the `Transition` — opens it, the
    /// same hand-off `pause_open` already uses.
    ask_discard: bool,
    /// Each data-driven list's scroll offset, echoed by its bind.
    scrolls: HashMap<&'static str, f64>,
    /// View settings the rig view reads (skeleton / base / collision / wireframe).
    show: [bool; 5],
    /// The gadget's handle colours, resolved from the theme once (they never change).
    gadget_style: GadgetStyle,
    /// The centre marks' colours, likewise.
    centre_style: CentreStyle,
    /// The Display group's backdrop toggle and the black it swaps the stages' grey for.
    backdrop_dark: bool,
    dark_backdrop: [f64; 4],
    /// SOLO — the Rig controls' own isolation verb. The PER-PANEL half (LIMB, CULL, CULL AT)
    /// is the panel's own chrome and is read back off it each frame.
    solo: bool,
    /// Which MARKER the rail has already selected and framed (`usize::MAX` = none yet). The rail
    /// moves on its own buttons only — never on a drag's release (incident 9715303C) — and a
    /// fresh rig re-opens it, so the scene FOLLOWS the document's cursor once per frame instead
    /// of each of them reaching in here.
    marker_shown: usize,
    /// THE SEVEN VIEW PANELS, each a `model_view` SUB SCENE (ruling EBDB3518: a nested
    /// surface hosts a COMPLETE scene) in `ui::view_slots()` order — the four quad panels,
    /// the preview step's bake view, the clip step's two variants. The bench places the slot
    /// and hands each panel its context by intent; the picture, the corner label / flip, the
    /// isolate row and the stage they draw under are the PANEL's own.
    views: Vec<SubScene<ModelView>>,
    /// The panel whose pointer the manipulator consumed last frame — that panel's camera is
    /// held still while the drag means a joint, not an orbit.
    gizmo_owned: Option<usize>,
    /// The GPU caches the panels' draw items come from.
    meshes: ViewMeshes,
    /// The pointer's picks and drags on the joints.
    gizmo_state: Gizmo,
    /// The clip step's clock (ticks of the active clip) and the preview's (the idle).
    clip_tick: f32,
    bake_tick: f32,
    /// The bake's skinning palette for this frame's pose, from `update` for `render`.
    bake_palette: Vec<Mat4>,
}

impl Clayworks {
    pub fn new(def: &SceneDef) -> Self {
        let ui_styles = flicker::ui::load_shared_styles(def.styles.as_ref());
        let tree = def
            .tree
            .clone()
            .expect("assetpipeline.scene.json declares a tree");
        let ui_intents = UiIntents::of(&tree);
        let script = ScriptHost::new(ui::SCRIPT, ui::SCRIPT_NAME)
            .expect("assetpipeline.lua loads (it ships with the crate)");
        // Each view surface names the `model_view` scene; the node's own `scene_*` props are
        // that instance's params, so which projection a panel shows — and whether it wears
        // the corner label and the isolate row — is authored beside the slot, in one place.
        // The def is the manifest's, the one `Goto{"model_view"}` would build from.
        let view_def = flicker_shell::scene_def(ui::VIEW_SCENE).unwrap_or_else(|| {
            panic!(
                "the `{}` scene is not in the manifest — the seven view surfaces name it",
                ui::VIEW_SCENE
            )
        });
        let views = ui::view_slots()
            .map(|(slot, _)| SubScene::new(ModelView::new(&view_def, &scene_params(&tree, slot))))
            .collect();
        Self {
            views,
            gizmo_owned: None,
            meshes: ViewMeshes::new(),
            gadget_style: compose::gadget_style(&ui_styles),
            centre_style: compose::centre_style(&ui_styles),
            backdrop_dark: false,
            dark_backdrop: compose::dark_backdrop(&ui_styles),
            solo: false,
            marker_shown: usize::MAX,
            gizmo_state: Gizmo::default(),
            clip_tick: 0.0,
            bake_tick: 0.0,
            bake_palette: Vec::new(),
            doc: Document::new(),
            tree,
            script,
            ui_styles,
            ui_state: UiState::default(),
            ui_intents,
            hud_commands: Vec::new(),
            theme: None,
            textures: Vec::new(),
            wf: Workflow::Character,
            tab: 0,
            ask_discard: false,
            scrolls: HashMap::new(),
            show: [true, true, false, false, true],
        }
    }

    /// Whether the VIEW pane holds the cursor — the gate on what the sub-scene panels may
    /// hear (contract §4d: a nested surface's discrete signals require focus).
    fn view_pane_focused(&self) -> bool {
        self.ui_state.focused_pane() == Some(ui::VIEW_PANE)
    }

    fn step(&self) -> Step {
        let steps = self.wf.steps();
        steps[self.tab.min(steps.len() - 1)]
    }

    /// A loaded, uncommitted source — leaving it costs the user's work.
    fn dirty(&self) -> bool {
        self.doc.source.is_some() && !self.doc.has_committed()
    }

    // ── Publish ─────────────────────────────────────────────────────────────

    /// The rows a `rows_from` list expands from — the document's data, labelled.
    fn rows(&self, source: &str) -> Option<Vec<Row>> {
        let r = |t: &str| strings::resolve(t).into_owned();
        Some(match source {
            ui::ROWS_PICKS | ui::ROWS_CLIPS => self
                .doc
                .candidate_rows()
                .into_iter()
                .map(|(stem, name)| Row::new(stem, name))
                .collect(),
            ui::ROWS_BONES => self
                .doc
                .bone_rows()
                .into_iter()
                .map(|(name, state)| {
                    let label = format!("{name}  {}", r(state.tag()));
                    Row::new(name, label)
                })
                .collect(),
            ui::ROWS_SOCKETS => self
                .doc
                .socket_rows()
                .into_iter()
                .map(|(id, token)| Row::new(id, r(&token)))
                .collect(),
            ui::ROWS_ATTACH => self
                .doc
                .attach_rows()
                .into_iter()
                .map(|(id, token)| Row::new(id, r(&token)))
                .collect(),
            // The regions' labels arrive pre-formatted (name · tag · anchor · chains · stiffness),
            // the tag already resolved where the row was composed.
            ui::ROWS_REGIONS => self
                .doc
                .region_rows()
                .into_iter()
                .map(|(name, label)| Row::new(name, label))
                .collect(),
            _ => return None,
        })
    }

    fn model(&self) -> ValueMap {
        let mut m = ValueMap::new();
        let step = self.step();
        m.set(ui::WF_BIND, self.wf.name());
        m.set(ui::TAB_BIND, self.tab as f64);
        m.set(ui::TABS_SHOWN, true);
        m.set(ui::STEP_TITLE, step.title());
        m.set(ui::STEP_HINT, step.hint());

        // The facts column: every readout PRE-FORMATTED.
        let count = |n: Option<usize>| n.map(|n| n.to_string()).unwrap_or_default();
        m.set(
            ui::ASSET_NAME,
            self.doc
                .asset_name()
                .map(str::to_string)
                .unwrap_or_else(|| "$ap_no_asset_loaded".to_string()),
        );
        m.set(ui::CLASS_LABEL, class_label(self.doc.class()).into_owned());
        m.set(ui::FACT_TRIS, count(self.doc.tri_count()));
        m.set(ui::FACT_VERTS, count(self.doc.vert_count()));
        m.set(ui::FACT_BONES, count(self.doc.bone_count()));
        m.set(ui::FACT_CLIPS, self.doc.clip_summary().unwrap_or_default());
        m.set(
            ui::FACT_STATUS,
            if self.doc.source.is_none() {
                "$ap_no_source_folder_open"
            } else if self.doc.has_committed() {
                "$ap_exported"
            } else {
                "$ap_not_exported"
            },
        );
        // The status line: an error first, else what conform did, else the open file.
        m.set(
            ui::STATUS,
            self.doc
                .error()
                .map(str::to_string)
                .or_else(|| self.doc.rig_summary())
                .or_else(|| self.doc.file_name().map(str::to_string))
                .unwrap_or_default(),
        );

        // Source.
        m.set(ui::PREFER_STAGED, self.doc.prefer_staged);
        m.set(ui::AS_PROVIDED, self.doc.as_provided);
        let picks = self.doc.candidate_rows();
        m.set(ui::HAS_PICKS, !picks.is_empty());
        m.set(ui::HAS_SOURCE, self.doc.source.is_some());
        m.set(
            ui::PICK_SEL,
            self.doc.selected_candidate().unwrap_or_default(),
        );

        // Prep.
        m.set(ui::STATURE, format!("{:.0}", self.doc.stature_cm));
        m.set(ui::DECIMATE, self.doc.decimate_target.clone());
        m.set(ui::FACING_READOUT, self.doc.facing_readout());
        m.set(
            ui::PREP_HEIGHT,
            Document::height_readout(self.doc.stature_cm),
        );
        m.set(ui::PREP_STATUS, self.doc.prep_status());
        m.set(ui::SKELETON_NAME, self.doc.skeleton_name());
        m.set(ui::SKELETON_SUMMARY, self.doc.skeleton_summary());
        // The MODULE controls read the working recipe (P2c S4): the first leg / arm / tail of
        // the root trunk and the head — what the Prep controls edit.
        let trunk = self.doc.recipe().trunk;
        let (legs, heel) = match trunk.legs.first() {
            Some(LegKind::Digitigrade { heel }) => (ui::LEG_VALUES[1], *heel),
            Some(LegKind::ToeWalker { heel, .. }) => (ui::LEG_VALUES[2], *heel),
            Some(LegKind::Unguligrade) => (ui::LEG_VALUES[3], 0.0),
            Some(LegKind::Bird { heel }) => (ui::LEG_VALUES[4], *heel),
            _ => (ui::LEG_VALUES[0], 0.0),
        };
        m.set(ui::LEGS, legs);
        m.set(ui::HEEL_PCT, format!("{:.0}", heel * 100.0));
        m.set(
            ui::ARMS,
            match trunk.arms.first() {
                Some(ArmKind::Hanging) => ui::ARM_VALUES[1],
                Some(ArmKind::Ungulate) => ui::ARM_VALUES[2],
                Some(ArmKind::Bird) => ui::ARM_VALUES[3],
                Some(ArmKind::Bat) => ui::ARM_VALUES[4],
                _ => ui::ARM_VALUES[0],
            },
        );
        let (tail, bones) = match trunk.tails.first() {
            None => (ui::TAIL_VALUES[0], 6),
            Some(TailKind::Short) => (ui::TAIL_VALUES[1], 6),
            Some(TailKind::ShortHair { bones }) => (ui::TAIL_VALUES[2], *bones),
            Some(TailKind::Long { bones }) => (ui::TAIL_VALUES[3], *bones),
        };
        m.set(ui::TAIL, tail);
        // SQUARE FROM (FEFDA2B2): the side the bake-time stance normaliser mirrors from.
        m.set(
            ui::STANCE,
            match self.doc.stance_source {
                StanceSource::Left => ui::STANCE_VALUES[1],
                StanceSource::Right => ui::STANCE_VALUES[2],
                StanceSource::Auto => ui::STANCE_VALUES[0],
            },
        );
        // MIRROR FROM (697DEC55) and FACE FORWARD (164AE2F3) — the other two source-shape knobs,
        // published beside the stance radio they replicate.
        m.set(
            ui::MIRROR_KEEP,
            match self.doc.mirror_keep {
                Some(Side::Left) => ui::MIRROR_KEEP_VALUES[1],
                Some(Side::Right) => ui::MIRROR_KEEP_VALUES[2],
                None => ui::MIRROR_KEEP_VALUES[0],
            },
        );
        m.set(ui::FACE_FORWARD, self.doc.face_forward);
        m.set(ui::TAIL_BONES, bones.to_string());
        // HANG (cm) — the region split's one measurement, typed like the stature beside it.
        m.set(ui::HANG_CM, format!("{:.0}", self.doc.hang_cm));
        m.set(ui::HEAD_ON, trunk.head);
        m.set(ui::PROBOSCIS_BONES, trunk.proboscis.to_string());
        m.set(
            ui::QUADRUPED,
            trunk.orientation == flicker_skeletal::format::Orientation::Quadruped,
        );

        // Rig.
        let bones = self.doc.bone_rows();
        m.set(
            ui::BONE_SEL,
            self.doc
                .bone_sel()
                .and_then(|i| bones.get(i))
                .map(|(name, _)| name.clone())
                .unwrap_or_default(),
        );
        let off = self.doc.selected_offset().unwrap_or_default();
        for (k, v) in ui::OFF.iter().zip(off.t) {
            m.set(*k, f64::from(v));
        }
        m.set(ui::OFF_ROLL, f64::from(off.roll));
        m.set(ui::GIZMO_MODE, self.gizmo_state.ui_mode().value());
        m.set(ui::GIZMO_SNAP, self.gizmo_state.snapping());
        m.set(ui::AUTO_DEPTH, self.gizmo_state.auto_depth());
        m.set(ui::MIRROR, self.doc.mirror_joints);
        // THE MARKERS RAIL's caption (spec FF40E825): the joint the bench is asking a human to
        // place. The four buttons carry their own labels; this is the one readout.
        m.set(ui::MARKER_PLACE, self.doc.marker_caption());
        // …and WHY it is asking for that one: what the shape graph matched (S2 431D08DF). A
        // read-only status line, exactly like the Prep readout above it — never a control.
        m.set(ui::MARKER_MATCH, self.doc.marker_match_caption());
        for (k, v) in ui::SHOW.iter().zip(self.show) {
            m.set(*k, v);
        }
        m.set(ui::SOLO, self.solo);
        m.set(ui::BACKDROP_DARK, self.backdrop_dark);

        // REGIONS (spec 0A81088E T2). The panel belongs to the two stops that have a body under
        // it: the Rig step, where a character's hair and tail are tagged, and Mount, where a
        // garment sits on the fitting body and can be split.
        m.set(ui::SHOWN_REGIONS, matches!(step, Step::Rig | Step::Mount));
        let region = self
            .doc
            .region_sel()
            .and_then(|i| self.doc.regions().get(i));
        m.set(
            ui::REGION_SEL,
            region.map(|r| r.name.clone()).unwrap_or_default(),
        );
        m.set(
            ui::REGION_TAG,
            region.map(|r| services::region_tag(r.tag).2).unwrap_or(""),
        );
        m.set(
            ui::REGION_BONE,
            region.map(|r| r.anchor_bone.clone()).unwrap_or_default(),
        );
        m.set(
            ui::REGION_CHAINS,
            region
                .map(|r| r.chain_count.to_string())
                .unwrap_or_default(),
        );
        m.set(
            ui::REGION_STIFFNESS,
            region.map_or(0.0, |r| f64::from(r.params.stiffness)),
        );
        let mapped = bones.iter().filter(|(_, s)| *s == MapState::Ok).count();
        m.set(
            ui::RIG_PROGRESS,
            if bones.is_empty() {
                0.0
            } else {
                mapped as f64 / bones.len() as f64
            },
        );

        // Mount.
        let sockets = self.doc.socket_rows();
        let fit = self.doc.fit().cloned().unwrap_or_default();
        m.set(
            ui::SOCK_SEL,
            sockets
                .get(fit.socket)
                .map(|(id, _)| id.clone())
                .unwrap_or_default(),
        );
        for (k, v) in ui::FIT_OFFSET.iter().zip(fit.offset) {
            m.set(*k, f64::from(v));
        }
        for (k, v) in ui::FIT_ROT.iter().zip(fit.rot) {
            m.set(*k, f64::from(v));
        }
        for (k, v) in ui::FIT_SCALE_AXES.iter().zip(fit.scale) {
            m.set(*k, f64::from(v));
        }
        m.set(ui::FIT_SCALE, f64::from(fit.uniform));

        // Preview.
        m.set(ui::PREVIEW_STATUS, self.doc.prep_status());

        // Attach.
        let attach = self.doc.attach_rows();
        m.set(
            ui::ATT_SEL,
            self.doc
                .attach_sel()
                .and_then(|i| attach.get(i))
                .map(|(id, _)| id.clone())
                .unwrap_or_default(),
        );
        let ao = self.doc.attach_offset().unwrap_or_default();
        for (k, v) in ui::ATT.iter().zip(ao) {
            m.set(*k, f64::from(v));
        }

        // Clip.
        m.set(ui::VARIANT_RM, self.doc.variant_rm);
        m.set(ui::VARIANT_IP, self.doc.variant_ip);

        // Review.
        let reqs = self.doc.requirements();
        for i in 0..ui::REQ_ROWS {
            let (ok, text) = reqs.get(i).cloned().unwrap_or((false, String::new()));
            m.set(ui::req_bind(i), text);
            m.set(
                ui::req_state_bind(i),
                if reqs.get(i).is_none() {
                    ""
                } else if ok {
                    "$ap_badge_passed"
                } else {
                    "$ap_badge_blocked"
                },
            );
        }
        m.set(ui::HAS_COMMITTED, self.doc.has_committed());
        m.set(ui::COMMIT_NOTE, self.doc.commit_note());

        // The lists' scroll offsets.
        for (_, bind) in ui::ROW_SOURCES {
            m.set(bind, self.scrolls.get(bind).copied().unwrap_or(0.0));
        }
        m
    }

    /// This frame's tree and Model: the document published, the data-driven rows
    /// expanded, and `arrange()`'s lit slices folded in.
    fn publish(&self) -> (UiNode, ValueMap) {
        let mut model = self.model();
        let tree = instantiate_rows(&self.tree, &mut model, &|source| self.rows(source));
        if let Err(e) = self.script.set_model(&model) {
            tracing::error!("clayworks: publishing the model to the script failed: {e}");
        }
        match self.script.arrange() {
            Ok(Some(arrangement)) => model.extend(arrangement.to_model()),
            Ok(None) => {}
            Err(e) => tracing::error!("clayworks: arrange() failed: {e}"),
        }
        (tree, model)
    }

    // ── Dispatch ────────────────────────────────────────────────────────────

    /// Open a folder into `workflow` — the Source step's four import buttons. The
    /// folder comes from the OPERATING SYSTEM's dialog through [`Document::pick_folder`]
    /// (Aaron's 2026-09-04 ruling AAD0DC4B: file selection is the OS dialog via the
    /// public `rfd` crate). A folder that opened raises the `loaded` signal for the
    /// script, which decides the next stop.
    fn import(&mut self, workflow: Workflow, class: Option<AssetClass>, prop: Option<PropKind>) {
        let Some(dir) = Document::pick_folder() else {
            return; // cancelled — stay put
        };
        self.doc.pending_class = class;
        self.doc.pending_prop = prop;
        self.doc.dispatch_workflow(match workflow {
            Workflow::Character => WF_CHARACTER,
            Workflow::Prop => WF_PROP,
            Workflow::Animation => WF_ANIMATION,
            Workflow::Creature => WF_CREATURE,
        });
        self.doc.open(dir);
        self.wf = workflow;
        self.tab = 0;
        self.ask_discard = false;
        self.scrolls.clear();
        if self.doc.source.is_some() {
            let mut sig = ValueMap::new();
            sig.set(ui::SIG_LOADED, true);
            sig.set(ui::WF_BIND, workflow.name());
            self.react(&sig);
        }
    }

    /// Hand a scene-level signal to the script's `react()` and fold what it returns into
    /// the ONE dispatcher — a `tab` write there IS a step change, so the script owns the
    /// flow's "what happens after" (the successor of the old workflow runtime's wf_next).
    fn react(&mut self, sig: &ValueMap) {
        match self.script.react(sig) {
            Ok(Some(intents)) => self.apply_results(&intents),
            Ok(None) => {}
            Err(e) => tracing::error!("clayworks: react() failed: {e}"),
        }
    }

    /// Move the rail to `tab` and run the services the stop needs — all idempotent
    /// (analyze no-ops once parsed, conform once rigged, prepare_clip once retargeted),
    /// so re-entering a stop costs nothing.
    fn go(&mut self, tab: usize) {
        self.tab = tab.min(self.wf.steps().len() - 1);
        tracing::debug!("clayworks: {} → {}", self.wf.name(), self.step().name());
        match self.step() {
            Step::Prep => {
                self.doc.analyze();
                self.doc.ensure_prep_source();
            }
            Step::Rig | Step::Mount => {
                self.doc.analyze();
                self.doc.conform();
                // THE RAIL OPENS THE RIG STEP (spec FF40E825): entering it selects and frames the
                // joint it is asking for, so the prompt is the first thing a human sees.
                self.marker_shown = usize::MAX;
            }
            Step::Clip => {
                self.doc.analyze();
                self.doc.conform();
                self.doc.prepare_clip();
            }
            Step::Source | Step::Preview | Step::Attach | Step::Review => {}
        }
    }

    /// THE ONE DISPATCHER: click results and fired intents, one map.
    fn apply_results(&mut self, r: &ValueMap) {
        // The unsaved-work answers: they arrive here from the SHARED modal through
        // `modal_closed`, folded into this ONE dispatcher exactly like a click — the
        // modal is a scene of its own now, so nothing has to gate the rest of the bench
        // on "a dialog is up".
        if r.is_on(ui::DISCARD_YES) {
            self.doc = Document::new();
            self.tab = 0;
            self.scrolls.clear();
            return;
        }
        if r.is_on(ui::DISCARD_NO) {
            return;
        }

        // Source: the four imports.
        if r.is_on(ui::IMPORT_CHARACTER) {
            self.import(Workflow::Character, Some(AssetClass::Skin), None);
        } else if r.is_on(ui::IMPORT_ACCESSORY) {
            self.import(
                Workflow::Prop,
                Some(AssetClass::Prop),
                Some(PropKind::Clothing),
            );
        } else if r.is_on(ui::IMPORT_PROP) {
            self.import(
                Workflow::Prop,
                Some(AssetClass::Prop),
                Some(PropKind::Environment),
            );
        } else if r.is_on(ui::IMPORT_ANIMATION) {
            self.import(Workflow::Animation, Some(AssetClass::Animation), None);
        } else if r.is_on(ui::IMPORT_CREATURE) {
            self.import(Workflow::Creature, Some(AssetClass::Creature), None);
        }

        // The rail: it steps ITSELF on `step_next` / `step_prev` (the rail owns its range);
        // only a CHANGED index moves the bench. Back off the first stop with work loaded
        // asks before it is lost.
        if let Some(v) = r.number(ui::TAB_BIND) {
            let want = (v.round().max(0.0) as usize).min(self.wf.steps().len() - 1);
            if want != self.tab {
                self.go(want);
            }
        }
        if r.is_on(ui::STEP_PREV) && self.tab == 0 && self.dirty() {
            self.ask_discard = true;
        }

        // Source settings and the candidate pick.
        if let Some(v) = r.get(ui::PREFER_STAGED).and_then(as_bool) {
            self.doc.prefer_staged = v;
        }
        if let Some(v) = r.get(ui::AS_PROVIDED).and_then(as_bool) {
            self.doc.as_provided = v;
        }
        if let Some(stem) = r.text(ui::PICK_SEL).filter(|s| !s.is_empty()) {
            if self.doc.selected_candidate() != Some(stem) {
                self.doc.select_candidate(stem);
            }
        }

        // Prep: the height is TYPED (Aaron 2026-09-07: a dial that lands where it likes is no way
        // to say 180) — a whole number of centimetres, 170 by default. The field STORES the
        // number; its APPLY button (or a submit) resizes the body — a re-opened rig included.
        let mut stature_changed = false;
        if let Some(t) = r.text(ui::STATURE) {
            if let Ok(cm) = t.trim().parse::<f32>() {
                if (40.0..=400.0).contains(&cm) && cm != self.doc.stature_cm {
                    self.doc.stature_cm = cm;
                    stature_changed = true;
                }
            }
        }
        if stature_changed || r.is_on(ui::STATURE_APPLY) {
            self.doc.apply_stature();
        }
        if let Some(t) = r.text(ui::DECIMATE) {
            if t != self.doc.decimate_target {
                self.doc.decimate_target = t.to_string();
            }
        }
        // HANG (cm): a typed whole number, stored the moment it parses — the SPLIT verb is its
        // apply, so there is no second button to press.
        if let Some(cm) = r
            .text(ui::HANG_CM)
            .and_then(|t| t.trim().parse::<f32>().ok())
        {
            if (0.0..=100.0).contains(&cm) {
                self.doc.hang_cm = cm;
            }
        }
        if r.is_on(ui::DECIMATE_APPLY) {
            self.doc.apply_decimate_target();
        }
        if r.is_on(ui::FACING_TURN) {
            self.doc.turn_facing();
        }
        self.apply_module_edits(r);
        if r.is_on(ui::DECIMATE_RESET) {
            self.doc.reset_decimate_target();
        }
        if r.is_on(ui::SKELETON_PREV) {
            self.doc.step_preset(-1);
        }
        if r.is_on(ui::SKELETON_NEXT) {
            self.doc.step_preset(1);
        }

        // Rig: the bone pick, its offsets, the mode, the toggles, the two verbs.
        if let Some(name) = r.text(ui::BONE_SEL).filter(|s| !s.is_empty()) {
            let cur = self
                .doc
                .bone_sel()
                .and_then(|i| self.doc.bone_rows().get(i).map(|(n, _)| n.clone()));
            if cur.as_deref() != Some(name) {
                self.doc.select_bone_named(name);
            }
        }
        if let Some(cur) = self.doc.selected_offset() {
            let off = BoneOffset {
                t: [
                    r.number(ui::OFF[0]).map_or(cur.t[0], |v| v as f32),
                    r.number(ui::OFF[1]).map_or(cur.t[1], |v| v as f32),
                    r.number(ui::OFF[2]).map_or(cur.t[2], |v| v as f32),
                ],
                roll: r.number(ui::OFF_ROLL).map_or(cur.roll, |v| v as f32),
                // The gadget's Scale writes this; no dial does, so it carries through.
                scale: cur.scale,
            };
            if off != cur {
                // The service mirrors the edit onto the twin bone when `mirror_joints` is
                // on — the dials and a gizmo drag share that one path.
                self.doc.set_selected_offset(off);
            }
        }
        if let Some(mode) = r.text(ui::GIZMO_MODE).and_then(GizmoUi::parse) {
            self.gizmo_state.set_ui_mode(mode);
        }
        if let Some(v) = r.get(ui::GIZMO_SNAP).and_then(as_bool) {
            self.gizmo_state.set_snap(v);
        }
        if let Some(v) = r.get(ui::AUTO_DEPTH).and_then(as_bool) {
            self.gizmo_state.set_auto_depth(v);
        }
        if let Some(v) = r.get(ui::MIRROR).and_then(as_bool) {
            self.doc.mirror_joints = v;
        }
        for (i, k) in ui::SHOW.iter().enumerate() {
            if let Some(v) = r.get(k).and_then(as_bool) {
                self.show[i] = v;
            }
        }
        if let Some(v) = r.get(ui::SOLO).and_then(as_bool) {
            self.solo = v;
        }
        if let Some(v) = r.get(ui::BACKDROP_DARK).and_then(as_bool) {
            self.backdrop_dark = v;
        }
        if r.is_on(ui::MIRROR_TO_TWIN) {
            if let Some(sel) = self.doc.bone_sel() {
                self.doc.mirror_to_twin(sel);
            }
        }
        if r.is_on(ui::FRAME_SEL) {
            self.frame_selection();
        }
        // THE MARKERS RAIL (spec FF40E825, ruling 42AB9BA8). The three step buttons MOVE THE RAIL
        // and nothing else — `show_marker` below selects the joint and frames the panels on it,
        // which is exactly the pair of verbs the human would have used by hand. `MARKER_NEXT` is
        // ACCEPT, the only way on (incident 9715303C). INFER is a button and derives the unplaced
        // joints in one go.
        for (key, step) in [
            (ui::MARKER_NEXT, MarkerStep::NextUnplaced),
            (ui::MARKER_SKIP, MarkerStep::Skip),
            (ui::MARKER_BACK, MarkerStep::Back),
        ] {
            if r.is_on(key) {
                self.doc.step_marker(step);
            }
        }
        if r.is_on(ui::MARKER_INFER) {
            self.doc.infer_markers();
        }
        if r.is_on(ui::BAKE_SKIN) {
            self.doc.bake_skin_now();
        }
        if r.is_on(ui::BONE_RESET) {
            // "Reset bone" is zeroing the authored correction (the posed skeleton derives).
            self.doc.set_selected_offset(BoneOffset::default());
        }
        self.apply_region_edits(r);

        // Mount: the socket pick and the fit dials.
        if let Some(id) = r.text(ui::SOCK_SEL).filter(|s| !s.is_empty()) {
            let sockets = self.doc.socket_rows();
            let cur = self
                .doc
                .fit()
                .and_then(|f| sockets.get(f.socket))
                .map(|(id, _)| id.clone());
            if cur.as_deref() != Some(id) {
                self.doc.select_socket(id);
            }
        }
        if let Some(fit) = self.doc.fit_mut() {
            for (k, v) in ui::FIT_OFFSET.iter().zip(fit.offset.iter_mut()) {
                if let Some(n) = r.number(k) {
                    *v = n as f32;
                }
            }
            for (k, v) in ui::FIT_ROT.iter().zip(fit.rot.iter_mut()) {
                if let Some(n) = r.number(k) {
                    *v = n as f32;
                }
            }
            for (k, v) in ui::FIT_SCALE_AXES.iter().zip(fit.scale.iter_mut()) {
                if let Some(n) = r.number(k) {
                    *v = n as f32;
                }
            }
            if let Some(n) = r.number(ui::FIT_SCALE) {
                fit.uniform = n as f32;
            }
        }

        // Attach: the point pick and its offset.
        if let Some(id) = r.text(ui::ATT_SEL).filter(|s| !s.is_empty()) {
            let attach = self.doc.attach_rows();
            let cur = self
                .doc
                .attach_sel()
                .and_then(|i| attach.get(i))
                .map(|(id, _)| id.clone());
            if cur.as_deref() != Some(id) {
                self.doc.select_attach(id);
            }
        }
        if let Some(cur) = self.doc.attach_offset() {
            let next = [
                r.number(ui::ATT[0]).map_or(cur[0], |v| v as f32),
                r.number(ui::ATT[1]).map_or(cur[1], |v| v as f32),
                r.number(ui::ATT[2]).map_or(cur[2], |v| v as f32),
            ];
            if next != cur {
                self.doc.set_attach_offset(next);
            }
        }

        // Clip: the variant picks.
        if let Some(v) = r.get(ui::VARIANT_RM).and_then(as_bool) {
            self.doc.variant_rm = v;
        }
        if let Some(v) = r.get(ui::VARIANT_IP).and_then(as_bool) {
            self.doc.variant_ip = v;
        }

        // Review: export, then the next piece of a multi-mesh folder.
        if r.is_on(ui::COMMIT) {
            self.doc.commit();
        }
        if r.is_on(ui::NEXT_PIECE) {
            self.doc.start_next_piece();
            let mut sig = ValueMap::new();
            sig.set(ui::SIG_NEXT_PIECE, true);
            self.react(&sig);
        }

        // The lists' scroll offsets (the list echoes its bind every frame).
        for (_, bind) in ui::ROW_SOURCES {
            if let Some(v) = r.number(bind) {
                self.scrolls.insert(bind, v);
            }
        }
    }
}

impl Clayworks {
    /// The Prep step's MODULE controls (P2c S4): each radio or field that differs from the
    /// working recipe edits it — legs (flat / heeled / toe walker, with the heel as a per cent
    /// of stature), arms, tail (and its bone count) and the head — from the picked preset as
    /// a starting point. The document rebuilds the prepped mesh on a change so the rig
    /// re-installs on the next Conform.
    fn apply_module_edits(&mut self, r: &ValueMap) {
        let trunk = self.doc.recipe().trunk;
        let heel_of = |leg: Option<&LegKind>| match leg {
            Some(LegKind::Digitigrade { heel })
            | Some(LegKind::ToeWalker { heel, .. })
            | Some(LegKind::Bird { heel }) => Some(*heel),
            _ => None,
        };
        let current_heel = heel_of(trunk.legs.first());
        if let Some(v) = r.text(ui::LEGS) {
            let want = match v {
                v if v == ui::LEG_VALUES[0] => Some(LegKind::Plantigrade),
                v if v == ui::LEG_VALUES[3] => Some(LegKind::Unguligrade),
                v if v == ui::LEG_VALUES[4] => Some(LegKind::Bird {
                    heel: current_heel.unwrap_or(0.25),
                }),
                v if v == ui::LEG_VALUES[1] => Some(LegKind::Digitigrade {
                    heel: current_heel.unwrap_or(0.15),
                }),
                v if v == ui::LEG_VALUES[2] => {
                    let LegKind::ToeWalker {
                        setback,
                        toe,
                        thigh,
                        calf,
                        ..
                    } = flicker_skeletal::format::Pattern::ToeWalker.leg()
                    else {
                        unreachable!("the toe walker pattern's leg is a toe walker")
                    };
                    Some(LegKind::ToeWalker {
                        heel: current_heel.unwrap_or(0.15),
                        setback,
                        toe,
                        thigh,
                        calf,
                    })
                }
                _ => None,
            };
            if let Some(want) = want {
                let same_kind = matches!(
                    (trunk.legs.first(), &want),
                    (Some(LegKind::Plantigrade), LegKind::Plantigrade)
                        | (
                            Some(LegKind::Digitigrade { .. }),
                            LegKind::Digitigrade { .. }
                        )
                        | (Some(LegKind::ToeWalker { .. }), LegKind::ToeWalker { .. })
                        | (Some(LegKind::Unguligrade), LegKind::Unguligrade)
                        | (Some(LegKind::Bird { .. }), LegKind::Bird { .. })
                );
                if !same_kind {
                    self.doc.edit_recipe(|t| t.legs = vec![want]);
                }
            }
        }
        if let Some(t) = r.text(ui::HEEL_PCT) {
            if let Ok(pct) = t.trim().parse::<f32>() {
                let heel = (pct / 100.0).clamp(0.02, 0.45);
                let trunk = self.doc.recipe().trunk;
                let differs =
                    heel_of(trunk.legs.first()).is_some_and(|h| ((h - heel) * 100.0).abs() >= 0.5);
                if differs {
                    self.doc.edit_recipe(|t| match t.legs.first_mut() {
                        Some(LegKind::Digitigrade { heel: h })
                        | Some(LegKind::ToeWalker { heel: h, .. })
                        | Some(LegKind::Bird { heel: h }) => *h = heel,
                        _ => {}
                    });
                }
            }
        }
        if let Some(v) = r.text(ui::ARMS) {
            let want = if v == ui::ARM_VALUES[1] {
                ArmKind::Hanging
            } else if v == ui::ARM_VALUES[2] {
                ArmKind::Ungulate
            } else if v == ui::ARM_VALUES[3] {
                ArmKind::Bird
            } else if v == ui::ARM_VALUES[4] {
                ArmKind::Bat
            } else {
                ArmKind::Humanoid
            };
            let trunk = self.doc.recipe().trunk;
            if trunk.arms.first() != Some(&want) {
                self.doc.edit_recipe(|t| t.arms = vec![want]);
            }
        }
        let bones = r
            .text(ui::TAIL_BONES)
            .and_then(|t| t.trim().parse::<u8>().ok())
            .map(|n| n.clamp(1, 16));
        if let Some(v) = r.text(ui::STANCE) {
            let want = match v {
                v if v == ui::STANCE_VALUES[1] => StanceSource::Left,
                v if v == ui::STANCE_VALUES[2] => StanceSource::Right,
                _ => StanceSource::Auto,
            };
            if want != self.doc.stance_source {
                self.doc.stance_source = want;
            }
        }
        // MIRROR FROM: a SOURCE-SHAPE knob, so a change re-preps the mesh from the pristine cache
        // (the facing knob's own pattern) rather than only being remembered for the bake.
        if let Some(v) = r.text(ui::MIRROR_KEEP) {
            self.doc.set_mirror_keep(match v {
                v if v == ui::MIRROR_KEEP_VALUES[1] => Some(Side::Left),
                v if v == ui::MIRROR_KEEP_VALUES[2] => Some(Side::Right),
                _ => None,
            });
        }
        if let Some(v) = r.get(ui::FACE_FORWARD).and_then(as_bool) {
            self.doc.face_forward = v;
        }
        if let Some(v) = r.text(ui::TAIL) {
            let trunk = self.doc.recipe().trunk;
            let current_bones = match trunk.tails.first() {
                Some(TailKind::ShortHair { bones }) | Some(TailKind::Long { bones }) => *bones,
                _ => bones.unwrap_or(6),
            };
            let want: Vec<TailKind> = match v {
                v if v == ui::TAIL_VALUES[1] => vec![TailKind::Short],
                v if v == ui::TAIL_VALUES[2] => vec![TailKind::ShortHair {
                    bones: current_bones,
                }],
                v if v == ui::TAIL_VALUES[3] => vec![TailKind::Long {
                    bones: current_bones,
                }],
                _ => Vec::new(),
            };
            let same_kind = matches!(
                (trunk.tails.first(), want.first()),
                (None, None)
                    | (Some(TailKind::Short), Some(TailKind::Short))
                    | (
                        Some(TailKind::ShortHair { .. }),
                        Some(TailKind::ShortHair { .. })
                    )
                    | (Some(TailKind::Long { .. }), Some(TailKind::Long { .. }))
            );
            if !same_kind {
                self.doc.edit_recipe(|t| t.tails = want);
            }
        }
        if let Some(n) = bones {
            let trunk = self.doc.recipe().trunk;
            let differs = matches!(
                trunk.tails.first(),
                Some(TailKind::ShortHair { bones }) | Some(TailKind::Long { bones }) if *bones != n
            );
            if differs {
                self.doc.edit_recipe(|t| match t.tails.first_mut() {
                    Some(TailKind::ShortHair { bones }) | Some(TailKind::Long { bones }) => {
                        *bones = n
                    }
                    _ => {}
                });
            }
        }
        if let Some(on) = r.get(ui::HEAD_ON).and_then(as_bool) {
            if on != self.doc.recipe().trunk.head {
                self.doc.edit_recipe(|t| t.head = on);
            }
        }
        if let Some(n) = r
            .text(ui::PROBOSCIS_BONES)
            .and_then(|t| t.trim().parse::<u8>().ok())
            .map(|n| n.min(16))
        {
            if n != self.doc.recipe().trunk.proboscis {
                self.doc.edit_recipe(|t| t.proboscis = n);
            }
        }
        if let Some(on) = r.get(ui::QUADRUPED).and_then(as_bool) {
            use flicker_skeletal::format::Orientation;
            let want = if on {
                Orientation::Quadruped
            } else {
                Orientation::Biped
            };
            if self.doc.recipe().trunk.orientation != want {
                self.doc.edit_recipe(|t| t.orientation = want);
            }
        }
    }

    /// FRAME: pan and zoom every rig panel onto the selected joint's subtree — the hand and
    /// its fingers — so it fills the panels; a lone joint frames at a fraction of the subject.
    /// The subject framing is untouched (a re-frame still returns to the whole body).
    /// THE MARKERS RAIL FOLLOWS ITS CURSOR: whenever the rail just moved — ACCEPT, SKIP or BACK,
    /// or a fresh rig re-opening it; never a drag's release (incident 9715303C) — the prompted
    /// joint becomes the SELECTION and the panels FRAME it, through the bench's own two verbs,
    /// which is exactly the pair a human would have used by hand. ONLY ON A CHANGE: re-selecting
    /// every frame would fight the free drag underneath, and that drag staying usable is the whole
    /// point of the rail being an opening phase rather than a mode. Returns whether it moved.
    fn show_marker(&mut self) -> bool {
        if self.marker_shown == self.doc.marker() {
            return false;
        }
        // The cursor is only recorded as SHOWN once it really has been: a body with no rail yet
        // (nothing conformed) must not latch position 0 and then stay silent when the rail arrives.
        let Some(name) = self.doc.marker_name() else {
            return false;
        };
        if !self.doc.select_bone_named(&name) {
            return false;
        }
        self.marker_shown = self.doc.marker();
        self.frame_selection();
        true
    }

    fn frame_selection(&mut self) {
        let Some(p) = self.doc.parsed() else {
            return;
        };
        let Some(sel) = self.doc.bone_sel() else {
            return;
        };
        let pts: Vec<Vec3> = p
            .subtree(sel)
            .iter()
            .filter_map(|&i| p.globals.get(i))
            .map(|g| g.w_axis.truncate())
            .collect();
        if pts.is_empty() {
            return;
        }
        let centre = pts.iter().sum::<Vec3>() / pts.len() as f32;
        let spread = pts
            .iter()
            .map(|q| (*q - centre).length())
            .fold(0.0, f32::max);
        let radius = (spread * 1.3).max(p.radius * FRAME_MIN_FRAC);
        // The four QUAD panels only: the bake and clip views show their own subject.
        for view in self.views.iter_mut().take(ui::RIG_SLOTS.len()) {
            view.scene_mut().focus(centre, radius);
        }
    }

    /// The armed GROW's press: the perspective panel's world ray against the working mesh, through
    /// the ONE ray-triangle the renderer owns (`flicker::render::ray_triangle`, front faces — the
    /// duplicate-kernel ruling B8D37267 says there is not to be a third). The nearest hit seeds the
    /// thin-part grow. Returns whether the press was SPENT — the arm goes either way, so a click
    /// that met nothing does not leave the panel armed behind the user's back.
    fn grow_from_click(&mut self, facts: &[PanelFacts]) -> bool {
        let Some((origin, dir)) = facts
            .first()
            .filter(|f| f.pointer.as_ref().is_some_and(|p| p.pressed))
            .and_then(|f| f.ray)
        else {
            return false;
        };
        self.doc.grow_armed = false;
        let Some(p) = self.doc.parsed() else {
            return true;
        };
        let hit = p
            .model
            .indices
            .as_chunks::<3>()
            .0
            .iter()
            .filter_map(|t| {
                let c = t.map(|i| {
                    p.model
                        .vertices
                        .get(i as usize)
                        .map(|v| Vec3::from_array(v.p))
                });
                match c {
                    [Some(a), Some(b), Some(c)] => {
                        flicker::render::ray_triangle(origin, dir, a, b, c)
                    }
                    _ => None,
                }
            })
            .fold(f32::INFINITY, f32::min);
        if hit.is_finite() {
            // The hit point lies IN the surface cell, which the rasteriser marked solid — which is
            // what the grow needs to start from.
            self.doc.grow_region(origin + dir * hit);
        }
        true
    }

    /// THE REGIONS PANEL (spec 0A81088E T2): the row pick, the four per-row knobs and the three
    /// verbs plus Remove. Every knob goes through `Document::edit_region` — the ONE seam that
    /// writes `model.regions[i]` and bumps the generation the highlight and the bake watch.
    ///
    /// SELECT CULLED reads the panels' live cut planes back off the three orthographic views (the
    /// isolation chrome is theirs since H3), so the selection is literally what they have cut away.
    /// GROW only ARMS here — its seed is the next perspective press, taken in `update` where the
    /// panels' rays exist.
    fn apply_region_edits(&mut self, r: &ValueMap) {
        if let Some(name) = r.text(ui::REGION_SEL).filter(|s| !s.is_empty()) {
            self.doc.select_region(name);
        }
        if let Some(i) = self.doc.region_sel() {
            let edits = [
                r.is_on(ui::REGION_TAG_PREV).then_some(RegionEdit::Tag(-1)),
                r.is_on(ui::REGION_TAG_NEXT).then_some(RegionEdit::Tag(1)),
                r.is_on(ui::REGION_BONE_PREV)
                    .then_some(RegionEdit::Anchor(-1)),
                r.is_on(ui::REGION_BONE_NEXT)
                    .then_some(RegionEdit::Anchor(1)),
                r.text(ui::REGION_CHAINS)
                    .map(|t| RegionEdit::Chains(t.to_string())),
                r.number(ui::REGION_STIFFNESS)
                    .map(|v| RegionEdit::Stiffness(v as f32)),
            ];
            for edit in edits.into_iter().flatten() {
                self.doc.edit_region(i, edit);
            }
            if r.is_on(ui::REGION_REMOVE) {
                self.doc.remove_region(i);
            }
        }
        if r.is_on(ui::REGION_SPLIT) {
            match self.doc.split_regions() {
                Ok(n) => tracing::info!("clayworks: the garment split into {n} regions"),
                // The status line is where a stage's failure is reported, as everywhere here.
                Err(e) => {
                    if let Some(s) = self.doc.source.as_mut() {
                        s.error = Some(e);
                    }
                }
            }
        }
        if r.is_on(ui::REGION_GROW) {
            self.doc.grow_armed = true;
        }
        if r.is_on(ui::REGION_CULLED) {
            let planes: Vec<(Vec3, f32)> = self.views[..ui::RIG_SLOTS.len()]
                .iter()
                .filter_map(|v| v.scene().cull_plane())
                .collect();
            self.doc.select_culled(&planes);
        }
    }
}

fn as_bool(v: &flicker::script::Value) -> Option<bool> {
    match v {
        flicker::script::Value::Bool(b) => Some(*b),
        _ => None,
    }
}

impl Scene for Clayworks {
    /// A shared modal closed over this bench: fold its answer into the ONE dispatcher,
    /// as a fired result name — the same channel a click arrives on, so `discard_yes` /
    /// `discard_no` mean here exactly what they meant when the dialog was an inline
    /// subtree. The payload is unused: a choice dialog collects nothing.
    fn modal_closed(&mut self, _modal: &str, result: &str, _payload: Option<&str>) {
        let mut r = ValueMap::new();
        r.set(result, true);
        self.apply_results(&r);
    }

    /// The decimate-target field owns the keyboard while its session is open — and so does a
    /// sub-scene panel's own field, while the view pane is the focused one (the panel is a whole
    /// scene: its context is as real as this one's).
    fn input_context(&self) -> Option<InputContext> {
        if self.ui_state.text_entry() {
            return Some(InputContext::TextEntry);
        }
        let focused = self.view_pane_focused();
        self.views.iter().find_map(|v| v.input_context(focused))
    }

    fn enter(&mut self, renderer: &mut Renderer) {
        renderer.clear_color = [0.02, 0.03, 0.05, 1.0];
        self.meshes.enter(renderer);
        let theme = Theme::build(renderer);
        let entries = theme.lua_textures();
        self.textures = entries.iter().map(|(_, h)| *h).collect();
        self.theme = Some(theme);
        // A sub scene never builds a Theme of its own — there is one per application and
        // this bench already owns it, so each panel draws its chrome from these very atlases.
        let textures = self.textures.clone();
        for view in &mut self.views {
            view.scene_mut().set_textures(textures.clone());
        }
    }

    fn update(
        &mut self,
        dt: Duration,
        input: &InputState,
        signals: &mut SceneInput,
        renderer: &Renderer,
    ) -> Transition {
        // A fresh profile's analog sensitivities ride this frame's context down to the panels.
        let controls = flicker_shell::take_pending_input().map(|(_map, look, _gp)| look);

        let screen = renderer.size();
        let (tree, model) = self.publish();
        // The gadget's allowed modes are AUTHORED: `arrange()` publishes one gate per mode for
        // the open step, and the ONE mode vocabulary turns those names into the gate. Applied
        // before this frame's results, so the radio a human just pressed is judged against the
        // step it was pressed on.
        self.gizmo_state.set_modes(modes_from_names(
            ui::GADGET_MODE_GATES
                .iter()
                .filter(|(gate, _)| model.is_on(gate))
                .map(|(_, name)| *name),
        ));
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
        let frame = run_ui(&tree, &model, &self.ui_styles, &snap, &mut self.ui_state);
        let over_hud = frame.results.is_on("hud_hit");
        // The walker RESERVED the view panels' rects; SEAT each panel in its own slot
        // (a dark step reserved nothing, so its panel seats `None` — no update, no render, no
        // cost). The slot's sample is the panel's whole pointer: the wrapper makes it local.
        let mut pointers = Vec::with_capacity(self.views.len());
        for (view, (slot, _)) in self.views.iter_mut().zip(ui::view_slots()) {
            view.seat(frame.surface(slot));
            pointers.push(frame.surface_pointer(slot).cloned());
        }
        self.hud_commands = frame.commands;

        let mut walker = WalkerHandler::hud(&mut self.ui_state, over_hud)
            .with_nav(&tree, &model)
            .with_intents(&self.ui_intents);
        {
            // The bench's OWN walker, and only it: every panel camera is inside a sub scene
            // now, which runs its own walker and its own dispatch on the signals the
            // wrapper hands it — but only while its surface is the focused pane (the
            // contract's §4d barrier, the ruling's "context from intent").
            let mut chain: [&mut dyn InputHandler; 1] = [&mut walker];
            Router::dispatch(signals.events, &mut chain, signals.route);
        }
        let mut results = frame.results;
        for name in walker.take_fired() {
            results.set(name, true);
        }
        drop(walker);
        self.apply_results(&results);

        // The clocks: the clip step's active clip and the preview's idle, in their own ticks.
        let dtf = dt.as_secs_f32();
        if let Some(cp) = self.doc.source.as_ref().and_then(|s| s.clip.as_ref()) {
            let hz = cp.ip.tick_rate_hz.max(1) as f32;
            self.clip_tick = (self.clip_tick + dtf * hz) % cp.duration.max(1) as f32;
        }
        if let Some(bp) = self.meshes.bake_ref() {
            let hz = bp.clip.tick_rate_hz.max(1) as f32;
            self.bake_tick = (self.bake_tick + dtf * hz) % bp.clip.duration_ticks.max(1) as f32;
        }

        // THE MANIPULATOR runs on the panels' facts, which only exist once each panel has
        // walked its own tree — so `interact` is the LAST thing this frame does (below), and
        // what it consumed rides into the NEXT frame's context as a held camera. A press
        // frame carries no travel, so nothing is lost by holding from the frame after.
        let step = self.step();
        let show = Show {
            skeleton: self.show[0],
            base: self.show[1],
            collision: self.show[2],
            wireframe: self.show[3],
            pbr: self.show[4],
        };
        let rig_composed = compose::rig_lines(&self.doc, show, step, self.meshes.base());
        let gizmo_active = step == Step::Rig && self.doc.bone_sel().is_some();
        let bake_composed = self.meshes.bake_ref().map(|bp| {
            let (globals, palette) = bp.pose(self.bake_tick);
            (compose::bake_lines(bp, &globals, show.skeleton), palette)
        });
        self.bake_palette = bake_composed
            .as_ref()
            .map(|(_, p)| p.clone())
            .unwrap_or_default();
        let clip_composed = self
            .doc
            .source
            .as_ref()
            .and_then(|s| s.clip.as_ref())
            .map(|cp| compose::clip_lines(cp, self.clip_tick));

        let look = GlobeWorld::look_from(|s| signals.axis(s, input));
        // The four rig panels share the VIEW pane, so the pane's cursor is what says a
        // discrete signal is meant for a panel at all (contract §4d).
        let focused = self.view_pane_focused();
        let rig_len = ui::RIG_SLOTS.len();
        // ISOLATION IS THE PANELS' OWN CHROME NOW: the bench reads each orthographic panel's
        // LIMB back off it, and the quad view's one current side is the SIDE panel's flip.
        let limb = [1, 2, 3].map(|i| self.views[i].scene().isolate().limb);
        let side = isolated_side(self.views[2].scene().flipped());
        let sel = self.doc.bone_sel();
        // Each panel's live cut, from the panel that owns it — drawn edge-on in the OTHERS.
        let planes: Vec<(usize, (Vec3, f32))> = self.views[..rig_len]
            .iter()
            .enumerate()
            .filter_map(|(i, v)| v.scene().cull_plane().map(|p| (i, p)))
            .collect();
        let rigged = self.doc.bone_count().unwrap_or(0) > 0;
        let (solo, backdrop, dark) = (self.solo, self.backdrop_dark, self.dark_backdrop);
        let held = self.gizmo_owned;
        for (i, view) in self.views.iter_mut().enumerate() {
            let projection = view.scene().projection();
            let composed = if i < rig_len {
                let mut c = if projection == Projection::Perspective {
                    rig_composed.clone()
                } else {
                    rig_composed.without_ground()
                };
                // ISOLATION: on the Rig step a panel under a filter swaps the shared skeleton
                // for its own — the orthographic panels hide, the picker dims. The Rig step's
                // overlay is the skeleton alone, so the swap is whole.
                if step == Step::Rig && show.skeleton && rigged {
                    if let Some(filter) = panel_filter(solo, limb, i, side, sel) {
                        let iso = compose::isolation(&self.doc, filter, i == 0);
                        c.overlay = compose::skeleton_overlay(&self.doc, &iso);
                    }
                }
                // The handles are PER PANEL — an orthographic view hides the axis it looks
                // along, and each handle wears its own Aim → Locked → Modify colour.
                if gizmo_active {
                    c.overlay.extend(
                        self.gizmo_state
                            .handle_lines(projection, &self.gadget_style),
                    );
                }
                // The world/model centre, and the OTHER panels' cut planes edge-on.
                c.overlay.extend(compose::centre_marks(
                    &c.framing,
                    projection,
                    &self.centre_style,
                ));
                let others: Vec<(Vec3, f32)> = planes
                    .iter()
                    .filter(|(j, _)| *j != i)
                    .map(|(_, p)| *p)
                    .collect();
                c.overlay.extend(compose::cut_marks(
                    &c.framing,
                    projection,
                    &others,
                    &self.centre_style,
                ));
                c
            } else if i == rig_len {
                match bake_composed.as_ref() {
                    Some((c, _)) => c.clone(),
                    None => compose::Composed {
                        lines: Vec::new(),
                        overlay: Vec::new(),
                        framing: rig_composed.framing,
                    },
                }
            } else {
                match clip_composed.as_ref() {
                    Some(pair) => pair[i - rig_len - 1].clone(),
                    None => compose::Composed {
                        lines: Vec::new(),
                        overlay: Vec::new(),
                        framing: rig_composed.framing,
                    },
                }
            };
            // CONTEXT BY INTENT — everything the panel should be LOOKING AT, handed over
            // before it walks. What it does with it (the camera, the flip, the cut, its own
            // chrome) is the panel's. The draws are the one piece that arrives in `render`,
            // where a `&mut Renderer` exists to upload them.
            view.scene_mut().set_context(ViewContext {
                draws: Vec::new(),
                lines: composed.lines,
                overlay: composed.overlay,
                centre: composed.framing.centre,
                radius: composed.framing.radius,
                half_extent: composed.framing.half,
                clear: backdrop.then_some(dark),
                // The pad's look belongs to the perspective rig panel, and only while the
                // VIEW pane is the focused one — the gate `RigView` used to apply from the
                // pane name it was built with, which a sub-scene panel (a whole scene, with no
                // pane of the host's) cannot know. The others take the pointer only.
                look: if i == 0 && focused {
                    look
                } else {
                    (0.0, 0.0, 0.0)
                },
                controls,
                camera_held: held == Some(i),
            });
            view.update(dt, input, pointers[i].as_ref(), signals, focused, renderer);
        }

        // THE MANIPULATOR, on what the panels now know: a projection, a world ray and the
        // panel's OWN root-surface sample — a press its chrome claimed never got that far.
        let facts: Vec<PanelFacts> = self.views[..rig_len]
            .iter()
            .map(|v| PanelFacts {
                projection: v.scene().projection(),
                ray: v.scene().ray(),
                pointer: v.scene().pointer().cloned(),
            })
            .collect();
        // GROW FROM CLICK (spec 0A81088E T2): while the verb is armed the next press in the
        // PERSPECTIVE panel is a seed, not a joint pick — so it is taken here, before the
        // manipulator, and that frame's gadget is held off so one press cannot mean two things.
        let grew = self.doc.grow_armed && self.grow_from_click(&facts);
        self.gizmo_owned = self.gizmo_state.interact(
            &mut self.doc,
            &facts,
            gizmo_active && !grew,
            rig_composed.framing.radius,
        );
        if self.step() == Step::Rig {
            self.show_marker();
        }

        // The unsaved-work prompt: the SHARED `choice_dialog` modal, opened by id with
        // the bench's own action names as its options and pushed exactly the way the
        // pause overlay is. Its answer comes back through `modal_closed`, below.
        if self.ask_discard {
            self.ask_discard = false;
            if let Some(theme) = self.theme {
                return Transition::Push(Box::new(SharedModal::open(
                    theme,
                    "choice_dialog",
                    // Keep-editing is also the cancel affordance, so Esc / pad-B backs
                    // out the SAFE way — a stray Escape never discards the work.
                    ModalParams::unsaved_changes(ui::DISCARD_YES, ui::DISCARD_NO)
                        .title("$wf_discard_title")
                        .body("$wf_discard_msg"),
                )));
            }
        }

        if results.is_on(ui::PAUSE_OPEN) {
            if let Some(theme) = self.theme {
                let pause_map = flicker_shell::input_profile()
                    .context_map("World")
                    .cloned()
                    .unwrap_or_else(InputMap::wasd_and_mouse);
                return Transition::Push(Box::new(PauseScene::new(
                    theme,
                    &pause_map,
                    &AbstractControls::default(),
                    &GamepadConfig::default(),
                )));
            }
        }
        Transition::None
    }

    fn exit(&mut self, renderer: &mut Renderer) {
        self.meshes.free(renderer);
        // Each panel exits its own scene and gives its render target back — a target
        // is an index into the renderer's pool, so dropping the wrapper reclaims nothing.
        for view in &mut self.views {
            view.exit(renderer);
        }
    }

    fn render<'f>(&'f mut self, renderer: &mut Renderer, fg: &mut FrameGraph<'f>) {
        // The draw items: uploaded through the caches (only when their keys moved), handed
        // to the panels as handles. The bake lives only on the preview step.
        let step = self.step();
        let show = Show {
            skeleton: self.show[0],
            base: self.show[1],
            collision: self.show[2],
            wireframe: self.show[3],
            pbr: self.show[4],
        };
        if step == Step::Preview {
            self.meshes.bake(&mut self.doc, renderer);
            // The bake's CPU cloth is stepped to the preview's own clip tick and written back
            // into its vertex buffer before anything draws it (spec 6C46CAB9).
            self.meshes
                .step_bake_cloth(renderer, &self.bake_palette, self.bake_tick);
        } else {
            self.meshes.release_bake(renderer);
        }
        let mut draws = compose::rig_draws(&self.doc, &mut self.meshes, renderer, step, show);
        // THE SELECTED REGION, over the subject: the wireframe twin's own pass over the region's
        // triangles, in the bench's selection amber — the very `stam_hi` token the selected joint
        // wears, resolved once into the gadget's palette (rule 790872EE: colours come from the ONE
        // palette). No renderer surface of its own; the panels cull and frame it like any mesh.
        if let Some(h) = self.meshes.region_mesh(&self.doc, renderer) {
            draws.push(Draw::Mesh {
                mesh: h,
                world: Mat4::IDENTITY,
                options: MeshDrawOptions {
                    tint: self.gadget_style.modifying,
                    wireframe: true,
                    ..Default::default()
                },
            });
        }
        for view in self.views.iter_mut().take(ui::RIG_SLOTS.len()) {
            view.scene_mut().set_draws(draws.clone());
        }
        let bake_draw = self
            .meshes
            .bake_ref()
            .filter(|_| !self.bake_palette.is_empty())
            .and_then(|bp| bp.draw(self.bake_palette.clone(), show.pbr));
        self.views[ui::RIG_SLOTS.len()]
            .scene_mut()
            .set_draws(bake_draw.into_iter().collect());

        let Self {
            views,
            hud_commands,
            textures,
            ..
        } = self;
        // Each seated panel declares its whole scene INSIDE its own target and composites it
        // where the slot was seated (base + the slot's layer, as before) — one blit apiece,
        // with the panel's chrome already inside the texture.
        let layer = fg.base_layer();
        for view in views.iter_mut() {
            view.render(renderer, fg, layer);
        }
        if let Some(&white) = textures.first() {
            fg.overlay(move |r| render_hud(r, hud_commands, white, textures));
        }
    }
}

/// Build the bench as a boxed `Scene` — the manifest resolves `assetpipeline.scene.json`
/// and hands its def here.
pub fn scene(def: &SceneDef) -> Box<dyn Scene> {
    Box::new(Clayworks::new(def))
}

#[cfg(test)]
mod tests {
    use super::*;
    use flicker::render::Vec2;

    fn bench() -> Clayworks {
        let def = SceneDef::parse("assetpipeline", ui::SCENE).expect("the shipped scene parses");
        Clayworks::new(&def)
    }

    /// Walk the bench's surface headlessly at a desktop size and return the frame.
    fn walk(b: &mut Clayworks) -> flicker::ui::UiFrame {
        let (tree, model) = b.publish();
        let snap = UiInput {
            mouse: Vec2::new(-1.0, -1.0),
            clicked: false,
            down: false,
            right_down: false,
            screen: Vec2::new(1600.0, 900.0),
            wheel: 0.0,
            exclusive: false,
            motion: Default::default(),
        };
        run_ui(&tree, &model, &b.ui_styles, &snap, &mut b.ui_state)
    }

    fn extent(frame: &flicker::ui::UiFrame, id: &str) {
        let r = frame
            .rect(id)
            .unwrap_or_else(|| panic!("`{id}` resolves to a rect"));
        assert!(
            r.size.x > 24.0 && r.size.y > 12.0,
            "`{id}` has extent: {:?}",
            r.size
        );
    }

    /// Walk the bench at an arbitrary SCREEN size, optionally with a wheel tick under
    /// the pointer — the short-window case the desktop-sized [`walk`] cannot reach.
    fn walk_on(b: &mut Clayworks, screen: Vec2, mouse: Vec2, wheel: f32) -> flicker::ui::UiFrame {
        let (tree, model) = b.publish();
        let snap = UiInput {
            mouse,
            clicked: false,
            down: false,
            right_down: false,
            screen,
            wheel,
            exclusive: false,
            motion: Default::default(),
        };
        run_ui(&tree, &model, &b.ui_styles, &snap, &mut b.ui_state)
    }

    /// Walk the bench with the mouse at `mouse`, pressing/holding as told.
    fn walk_at(b: &mut Clayworks, mouse: Vec2, clicked: bool, down: bool) -> flicker::ui::UiFrame {
        let (tree, model) = b.publish();
        let snap = UiInput {
            mouse,
            clicked,
            down,
            right_down: false,
            screen: Vec2::new(1600.0, 900.0),
            wheel: 0.0,
            exclusive: false,
            motion: Default::default(),
        };
        run_ui(&tree, &model, &b.ui_styles, &snap, &mut b.ui_state)
    }

    /// THE FIRST PRESS INTO AN ORTHOGRAPHIC PANEL IS THAT PANEL'S (Aaron 2026-09-07: "You have to
    /// first actively click into an orthagonal view for it to do anything"): through the real
    /// tree at the Rig step, the very first press over the Top panel arrives on the slot's
    /// pointer sample as a captured press — no focusing click is owed first — and so does a
    /// second.
    ///
    /// Since H3 that sample is exactly what the bench hands the SUB-SCENE panel
    /// (`SubScene::update`'s `pointer`), so this gate covers the bench's whole half of the
    /// press. The child's half — the same sample rewritten into local coordinates, and its
    /// own walker's barrier over its own chrome — is `flicker_shell::sub_scene`'s
    /// `a_sample_addresses_the_pointer_at_the_surface_in_local_coordinates` and
    /// `flicker_modelview`'s tree tests. A sub-scene panel cannot be driven here: it enters on
    /// its first seated RENDER, which needs a GPU.
    #[test]
    fn the_first_press_into_an_ortho_panel_is_the_panels() {
        let mut b = bench();
        b.doc = crate::tests::synthetic_rigged_doc("first_press");
        b.tab = b.wf.steps().iter().position(|s| *s == Step::Rig).unwrap();
        let idle = walk(&mut b);
        let slot = idle
            .surface("ap_view_top")
            .expect("the top panel is reserved on Rig");
        let centre = Vec2::new(slot.x + slot.w * 0.5, slot.y + slot.h * 0.5);
        let sample = |f: &flicker::ui::UiFrame| {
            f.surface_pointer("ap_view_top")
                .map(|p| (p.pressed, p.captured, p.left))
        };
        let _ = walk_at(&mut b, centre, false, false);
        let first = walk_at(&mut b, centre, true, true);
        assert_eq!(
            sample(&first),
            Some((true, true, true)),
            "the first press is the panel's"
        );
        let held = walk_at(&mut b, centre + Vec2::new(0.0, -10.0), false, true);
        assert_eq!(sample(&held), Some((false, true, true)), "and it holds");
        let _ = walk_at(&mut b, centre + Vec2::new(0.0, -10.0), false, false);
        let second = walk_at(&mut b, centre, true, true);
        assert_eq!(
            sample(&second),
            Some((true, true, true)),
            "so is the second"
        );
    }

    // A PRESS ON A PANEL'S OWN CHROME NEVER REACHES THE PANEL — covered where the chrome now
    // lives. The isolate row and the corner label are the SUB SCENE's own nodes since H3, so
    // the barrier that keeps a tick on LIMB out of the camera is the child's walker:
    // `flicker_modelview`'s `the_isolate_binds_echo_back_into_the_model` drives that press
    // through the real tree, and `flicker_shell::sub_scene` pins the local sample it rides. The
    // bench has no panel chrome left to press.

    /// THE QUAD VIEW HAS ONE CURRENT SIDE — the side panel's flip — and FRONT/TOP follow it; SOLO
    /// wins wherever a joint is selected; the perspective picker follows any isolating panel.
    #[test]
    fn front_and_top_follow_the_side_panel_and_solo_wins() {
        assert_eq!(
            isolated_side(false),
            Side::Left,
            "LEFT shows the left limbs"
        );
        assert_eq!(isolated_side(true), Side::Right, "a flip switches the side");
        // The three ortho panels' own LIMB checkboxes, as the bench reads them back off each
        // panel (`ModelView::isolate`) — only the side panel isolates here.
        let one = [false, true, false];
        assert_eq!(panel_filter(false, one, 1, Side::Right, Some(3)), None);
        assert_eq!(
            panel_filter(false, one, 2, Side::Right, Some(3)),
            Some(BoneFilter::Limb(Side::Right))
        );
        assert_eq!(panel_filter(false, one, 3, Side::Right, Some(3)), None);
        assert_eq!(
            panel_filter(false, one, 0, Side::Right, Some(3)),
            Some(BoneFilter::Limb(Side::Right)),
            "the picker follows"
        );
        let all = [true; 3];
        for panel in 1..4 {
            assert_eq!(
                panel_filter(false, all, panel, Side::Left, None),
                Some(BoneFilter::Limb(Side::Left)),
                "front and top take the side panel's side"
            );
        }
        // SOLO is the BENCH's own control — the one half of the isolation that did not move
        // into the panels — and it wins wherever a joint is selected.
        assert_eq!(
            panel_filter(true, all, 3, Side::Left, Some(7)),
            Some(BoneFilter::Subtree(7)),
            "solo wins with a selection"
        );
        assert_eq!(
            panel_filter(true, all, 3, Side::Left, None),
            Some(BoneFilter::Limb(Side::Left)),
            "and falls back to the limb without one"
        );
        assert_eq!(
            panel_filter(false, [false; 3], 2, Side::Left, Some(1)),
            None
        );
        assert_eq!(
            flicker_modelview::Isolate::default().cull_at,
            1.0,
            "a fresh cut cuts nothing"
        );
    }

    /// THE IMPORT ARM CALLS THE PICK SEAM, and what the seam returns is what opens.
    ///
    /// File selection is the OS dialog through `rfd` (Aaron's ruling AAD0DC4B); the
    /// dialog call is factored behind [`Document::pick_folder`], whose `#[cfg(test)]`
    /// arm answers from an armed stub, so this gate drives the REAL Source-step path —
    /// button → `import()` → `pick_folder()` → `Document::open` — without any test
    /// ever opening a native dialog.
    #[test]
    fn the_import_calls_the_pick_seam_and_opens_what_it_returns() {
        let fixture = crate::tests::synth_source_dir("os_pick");
        let mut b = bench();
        let mut r = ValueMap::new();
        r.set(ui::IMPORT_CHARACTER, true);

        // 1 — a CANCELLED pick (the stub is unarmed) stays put: nothing opens, nothing
        // dispatches, the bench does not move off the Source step.
        b.apply_results(&r);
        assert!(b.doc.source.is_none(), "a cancelled pick opens nothing");
        assert_eq!(b.wf, Workflow::Character, "nothing dispatched");
        assert_eq!(b.tab, 0);

        // 2 — an ANSWERED pick opens the folder the seam returned, into the workflow the
        // button carried. The stub is consumed once, which is the proof the arm CALLED it.
        crate::services::stub_pick(fixture.clone());
        b.apply_results(&r);
        let source = b.doc.source.as_ref().expect("the picked folder opened");
        assert_eq!(source.dir, fixture, "on the folder the dialog returned");
        assert_eq!(
            b.wf,
            Workflow::Character,
            "into the workflow the button named"
        );
        assert!(
            Document::pick_folder().is_none(),
            "the arm consumed the seam's answer — one press, one dialog"
        );
        let _ = std::fs::remove_dir_all(&fixture);
    }

    /// THE MARKERS RAIL (spec FF40E825, ruling 42AB9BA8) is the Rig step's OPENING PHASE: it names
    /// the joint it wants, SELECTS it and FRAMES the panels on it, and the free drag underneath is
    /// what places it. ACCEPT (the `MARKER_NEXT` node) goes to the next joint that still wants a
    /// human — the only way on, a release never moves the rail (incident 9715303C, gated in
    /// `gizmo.rs`) — SKIP and BACK step the list, INFER is a button. WHICH joints it asks for is
    /// the shape matcher's answer (S2
    /// 431D08DF) — gated in `tests.rs` with the PLACED signal and `infer_markers`' own geometry;
    /// this is the wiring, the layout and the copy.
    #[test]
    fn the_markers_rail_prompts_selects_and_frames_the_joint_it_asks_for() {
        crate::tests::load_shipped_strings();
        let mut b = bench();
        b.doc = crate::tests::synthetic_rigged_doc("markers_rail");
        b.tab = 2; // the Rig step

        // THE PROMPT: the joints the MATCHER left for the human, and the caption is the
        // stringtable's sentence with the joint name in it as data. The fixture is a ball — one
        // core, so the trunk module matches it and its `pelvis` is placed, not prompted.
        let markers = b.doc.markers();
        let first = markers.first().cloned().unwrap_or_default();
        assert!(
            b.doc.shape_match().is_some(),
            "the raw-mesh fit read the fixture's shape"
        );
        assert!(!first.is_empty(), "and left the human some joints to place");
        let caption = b
            .publish()
            .1
            .text(ui::MARKER_PLACE)
            .unwrap_or_default()
            .to_string();
        assert!(
            !caption.starts_with('$') && caption.contains(&first),
            "the caption resolves and names the joint: {caption}"
        );
        // THE MATCH STATUS beside it — the one read-only line saying why that joint and not the
        // pelvis. Resolved and formatted where every other readout is.
        let status = b
            .publish()
            .1
            .text(ui::MARKER_MATCH)
            .unwrap_or_default()
            .to_string();
        assert!(
            !status.is_empty() && !status.starts_with('$') && !status.contains('{'),
            "the match status resolves and takes its counts: {status}"
        );

        // IT SELECTS AND FRAMES — the two verbs the human would have used by hand.
        assert!(b.show_marker(), "the rail's first prompt is shown");
        assert_eq!(
            b.doc
                .bone_sel()
                .and_then(|i| b.doc.bone_rows().get(i).map(|(n, _)| n.clone())),
            Some(first.clone()),
            "the prompted joint IS the selection"
        );
        assert!(!b.show_marker(), "and is not re-selected while it stands");

        // THE THREE STEPS. SKIP walks the list; BACK comes back; ACCEPT goes to the next joint
        // that still wants a human — with nothing placed, that is simply the one after.
        let step = |b: &mut Clayworks, key: &str| {
            let mut r = ValueMap::new();
            r.set(key, true);
            b.apply_results(&r);
            b.doc.marker_name().unwrap_or_default()
        };
        assert_eq!(step(&mut b, ui::MARKER_SKIP), markers[1]);
        assert_eq!(step(&mut b, ui::MARKER_BACK), markers[0]);
        assert_eq!(step(&mut b, ui::MARKER_NEXT), markers[1]);
        assert!(b.show_marker(), "and the rail follows its cursor");

        // INFER is a BUTTON, and it derives without ever marking anything placed.
        let mut r = ValueMap::new();
        r.set(ui::MARKER_INFER, true);
        b.apply_results(&r);
        assert!(
            !b.doc.placed().iter().any(|p| *p),
            "Infer never places a joint on the human's behalf"
        );

        let frame = walk(&mut b);
        for id in [
            ui::MARKER_NEXT,
            ui::MARKER_BACK,
            ui::MARKER_SKIP,
            ui::MARKER_INFER,
            ui::MARKER_PLACE,
            ui::MARKER_MATCH,
        ] {
            extent(&frame, id);
        }
        for key in [
            "$ap_marker_place",
            "$ap_marker_accept",
            "$ap_marker_back",
            "$ap_marker_skip",
            "$ap_marker_infer",
            "$ap_marker_match",
            "$ap_marker_match_all",
        ] {
            let t = flicker::ui::strings::resolve(key);
            assert!(!t.starts_with('$'), "{key} resolves, got {t}");
        }
    }

    /// MIRROR FROM (697DEC55) and FACE FORWARD (164AE2F3) ride with the body exactly as SQUARE
    /// FROM does: the radio and the checkbox publish the document's values and a pressed value
    /// comes back onto them. The two verbs' own behaviour is gated in flicker-content
    /// (`mirror_mesh` / `face_forward`) and their APPLICATION in `tests.rs`; this is the wiring,
    /// and that the five labels are real strings. FACE FORWARD opens CHECKED (A79A6131) — the
    /// checkbox is the opt-out.
    #[test]
    fn the_prep_mirror_and_face_controls_carry_the_source_shape_knobs() {
        crate::tests::load_shipped_strings();
        let mut b = bench();
        assert_eq!(
            b.doc.mirror_keep, None,
            "Off by default — a mirror destroys"
        );
        assert!(
            b.doc.face_forward,
            "and a turned head is faced forward by default"
        );
        let (_, m) = b.publish();
        assert_eq!(m.text(ui::MIRROR_KEEP), Some(ui::MIRROR_KEEP_VALUES[0]));
        assert_eq!(m.get(ui::FACE_FORWARD).and_then(as_bool), Some(true));
        for (value, want) in [
            (ui::MIRROR_KEEP_VALUES[1], Some(Side::Left)),
            (ui::MIRROR_KEEP_VALUES[2], Some(Side::Right)),
            (ui::MIRROR_KEEP_VALUES[0], None),
        ] {
            let mut r = ValueMap::new();
            r.set(ui::MIRROR_KEEP, value);
            b.apply_results(&r);
            assert_eq!(b.doc.mirror_keep, want, "`{value}` reaches the document");
            let (_, m) = b.publish();
            assert_eq!(
                m.text(ui::MIRROR_KEEP),
                Some(value),
                "and is published back"
            );
        }
        for want in [false, true] {
            let mut r = ValueMap::new();
            r.set(ui::FACE_FORWARD, want);
            b.apply_results(&r);
            assert_eq!(
                b.doc.face_forward, want,
                "the checkbox reaches the document"
            );
            let (_, m) = b.publish();
            assert_eq!(m.get(ui::FACE_FORWARD).and_then(as_bool), Some(want));
        }
        for key in [
            "$ap_mirror_keep",
            "$ap_mirror_off",
            "$ap_mirror_left",
            "$ap_mirror_right",
            "$ap_face_forward",
        ] {
            let t = flicker::ui::strings::resolve(key);
            assert!(!t.starts_with('$'), "{key} resolves, got {t}");
        }
    }

    /// SQUARE FROM (Aaron's ruling FEFDA2B2) rides with the body like the facing knob: the Prep
    /// radio publishes the document's stance source and a pressed value comes back onto it, which
    /// is what the ONE bake path reads. The normaliser's own behaviour is gated in flicker-content
    /// (`square_stance`); this is the wiring, and that the three labels are real strings.
    #[test]
    fn the_prep_square_from_radio_carries_the_stance_source() {
        crate::tests::load_shipped_strings();
        let mut b = bench();
        assert_eq!(b.doc.stance_source, StanceSource::Auto, "Auto by default");
        let (_, m) = b.publish();
        assert_eq!(m.text(ui::STANCE), Some(ui::STANCE_VALUES[0]));
        for (value, want) in [
            (ui::STANCE_VALUES[1], StanceSource::Left),
            (ui::STANCE_VALUES[2], StanceSource::Right),
            (ui::STANCE_VALUES[0], StanceSource::Auto),
        ] {
            let mut r = ValueMap::new();
            r.set(ui::STANCE, value);
            b.apply_results(&r);
            assert_eq!(b.doc.stance_source, want, "`{value}` reaches the document");
            let (_, m) = b.publish();
            assert_eq!(m.text(ui::STANCE), Some(value), "and is published back");
        }
        for key in [
            "$ap_stance",
            "$ap_stance_auto",
            "$ap_stance_left",
            "$ap_stance_right",
        ] {
            let t = flicker::ui::strings::resolve(key);
            assert!(!t.starts_with('$'), "{key} resolves, got {t}");
        }
    }

    /// THE REGIONS PANEL, end to end at the bench (spec 0A81088E T2): it stands on the two stops
    /// that have a body under them, its knobs reach the document through the ONE seam, the row it
    /// publishes is the row the list shows, and every string it wears comes out of the table.
    #[test]
    fn the_regions_panel_reaches_the_document_and_speaks_the_table() {
        crate::tests::load_shipped_strings();
        let mut b = bench();
        b.doc = crate::tests::synthetic_rigged_doc("regions_panel");

        // The gate: Rig and Mount, and nowhere else.
        for (tab, want) in [(0, false), (1, false), (2, true), (3, false)] {
            b.tab = tab;
            assert_eq!(b.publish().1.is_on(ui::SHOWN_REGIONS), want, "tab {tab}");
        }
        b.wf = Workflow::Prop;
        b.tab = 1; // the prop rail's Mount stop
        assert!(
            b.publish().1.is_on(ui::SHOWN_REGIONS),
            "Mount carries it too"
        );
        b.wf = Workflow::Character;
        b.tab = 2;

        // SELECT CULLED with no cut selects nothing; with the panels' cut it makes a row.
        let mut r = ValueMap::new();
        r.set(ui::REGION_CULLED, true);
        b.apply_results(&r);
        assert!(
            b.doc.regions().is_empty(),
            "no panel is cutting, so nothing is hidden"
        );
        assert!(b.doc.select_culled(&[(Vec3::Z, 20.0)]));

        // The row and its knobs publish as the list and the steppers read them.
        let (_, m) = b.publish();
        let name = b.doc.regions()[0].name.clone();
        assert_eq!(m.text(ui::REGION_SEL), Some(name.as_str()));
        assert_eq!(m.text(ui::REGION_TAG), Some("$ap_region_cloth"));
        assert_eq!(m.text(ui::REGION_CHAINS), Some("1"));
        assert_eq!(
            m.text(ui::REGION_BONE),
            Some(b.doc.regions()[0].anchor_bone.as_str())
        );

        // Each knob's own result reaches `edit_region` and bumps the generation.
        for (key, check) in [
            (ui::REGION_TAG_NEXT, "tag"),
            (ui::REGION_BONE_NEXT, "anchor"),
        ] {
            let gen = b.doc.region_gen;
            let mut r = ValueMap::new();
            r.set(key, true);
            b.apply_results(&r);
            assert!(b.doc.region_gen > gen, "{check} reached the document");
        }
        let mut r = ValueMap::new();
        r.set(ui::REGION_CHAINS, "3");
        r.set(ui::REGION_STIFFNESS, 0.02_f64);
        b.apply_results(&r);
        assert_eq!(b.doc.regions()[0].chain_count, 3);
        assert!((b.doc.regions()[0].params.stiffness - 0.02).abs() < 1e-6);

        // GROW only ARMS from the button — its seed is the next perspective press.
        let mut r = ValueMap::new();
        r.set(ui::REGION_GROW, true);
        b.apply_results(&r);
        assert!(b.doc.grow_armed, "the next perspective click is the seed");

        // REMOVE drops the row.
        let mut r = ValueMap::new();
        r.set(ui::REGION_REMOVE, true);
        b.apply_results(&r);
        assert!(b.doc.regions().is_empty());

        // HANG (cm) rides with the document like the stance source does, and it LIVES HERE — in
        // the panel whose SPLIT verb reads it — so the Prop/garment rail, which has no Prep page
        // at all, finally reaches it on MOUNT and edits the very same value (T2's owed item 2).
        let mut r = ValueMap::new();
        r.set(ui::HANG_CM, "8");
        b.apply_results(&r);
        assert!((b.doc.hang_cm - 8.0).abs() < 1e-6);
        assert_eq!(b.publish().1.text(ui::HANG_CM), Some("8"));
        b.wf = Workflow::Prop;
        b.tab = 1; // the garment rail's Mount stop
        assert_eq!(
            b.publish().1.text(ui::HANG_CM),
            Some("8"),
            "Mount publishes the same document value"
        );
        let frame = walk(&mut b);
        extent(&frame, ui::HANG_CM);
        let mut r = ValueMap::new();
        r.set(ui::HANG_CM, "14");
        b.apply_results(&r);
        assert!(
            (b.doc.hang_cm - 14.0).abs() < 1e-6,
            "and edits it, which is what `write_garment` bakes with"
        );
        b.wf = Workflow::Character;
        b.tab = 2;

        for key in [
            "$ap_regions",
            "$ap_hang_cm",
            "$ap_region_chains",
            "$ap_region_stiffness",
            "$ap_region_split",
            "$ap_region_grow",
            "$ap_region_culled",
            "$ap_region_remove",
            "$ap_region_cloth",
            "$ap_region_hair",
            "$ap_region_mane",
            "$ap_region_tail",
            "$ap_region_pendant",
            "$ap_region_appendage",
            "$ap_proboscis_bones",
            "$ap_pbr",
            "$ap_rig_fitted",
            "$ap_rig_reopened",
        ] {
            let t = flicker::ui::strings::resolve(key);
            assert!(!t.starts_with('$'), "{key} resolves, got {t}");
        }
    }

    /// THE CHARACTER RAIL FITS ITS SKELETON (incident 118CEA35: Aaron — "on the rigging stage it
    /// didn't seem to do any attempt to apply the skeleton"): a boneless character opened on the
    /// Character rail reaches the Rig step with the composed skeleton FITTED by the one
    /// shape-graph fit — the match the rail reads, a status line that says what was fitted (never
    /// the vendor conform's "0 inferred"), every bone a row — and the frames' own read-backs
    /// leave it standing.
    #[test]
    fn a_character_reaches_the_rig_step_with_its_skeleton_fitted_and_says_so() {
        crate::tests::load_shipped_strings();
        let mut b = bench();
        b.doc.pending_class = Some(AssetClass::Skin);
        b.doc
            .open(crate::tests::synth_source_dir("character_rig_step"));
        {
            let src = b.doc.source.as_mut().expect("the scratch folder opened");
            src.parsed = Some(crate::services::Parsed::new(crate::tests::sphere_mesh(
                6, 8, 50.0,
            )));
            src.error = None;
        }
        b.wf = Workflow::Character;
        b.go(1);
        let _ = walk(&mut b);
        assert_eq!(b.doc.bone_count().unwrap_or(0), 0, "Prep installs nothing");
        b.go(2);
        for _ in 0..3 {
            let _ = walk(&mut b);
        }
        assert_eq!(b.step(), Step::Rig);
        assert!(b.doc.error().is_none(), "no error: {:?}", b.doc.error());
        assert_eq!(
            b.doc.bone_count().unwrap_or(0) + 1,
            b.doc.recipe_bones(),
            "the composed humanoid is installed and stays through the frames"
        );
        let m = b
            .doc
            .shape_match()
            .expect("the fit's own match reaches the rail");
        assert!(!m.matched.is_empty(), "the fit matched the body");
        let (_, model) = b.publish();
        let status = model.text(ui::STATUS).unwrap_or_default().to_string();
        assert!(
            status.contains("fitted") && status.contains("matched"),
            "the status speaks the fit: {status:?}"
        );
        assert!(
            !status.contains("Inferred"),
            "never the vendor conform's line: {status:?}"
        );
        assert_eq!(
            b.doc.bone_rows().len(),
            b.doc.bone_count().unwrap_or(0),
            "every bone is a row of the Rig step"
        );
    }

    /// The Source step shows the four import cards with real extent; the Prep step shows
    /// the stature dial, the target field and its two verbs; the Rig step reserves the
    /// four view panels — presence AND extent, the twice-burned lesson.
    #[test]
    fn the_rebuilt_surface_lays_out_every_step_with_extent() {
        let mut b = bench();
        let frame = walk(&mut b);
        for id in [
            ui::IMPORT_CHARACTER,
            ui::IMPORT_ACCESSORY,
            ui::IMPORT_PROP,
            ui::IMPORT_ANIMATION,
        ] {
            extent(&frame, id);
        }
        assert!(
            frame.rect(ui::DECIMATE).is_none(),
            "prep controls are dark on Source"
        );
        for (slot, _) in ui::RIG_SLOTS {
            assert!(
                frame.surface(slot).is_none(),
                "{slot} is unreserved on Source"
            );
        }

        // The step rail itself: the paged menu places it only while the bench publishes
        // `paged_tabs_shown` — a missing flag collapses it and every step verb with it.
        extent(&frame, "ap_steps_character");

        b.tab = 1; // prep
        let frame = walk(&mut b);
        extent(&frame, ui::STATURE);
        extent(&frame, ui::DECIMATE);
        extent(&frame, ui::DECIMATE_RESET);
        extent(&frame, ui::DECIMATE_APPLY);
        extent(&frame, ui::SKELETON_PREV);
        extent(&frame, ui::SKELETON_NEXT);
        // The MODULE controls (P2c S4): every radio, field and the head checkbox lays out.
        for id in [
            "legs_flat",
            "legs_heeled",
            "legs_toewalker",
            "legs_hooves",
            "legs_bird",
            ui::HEEL_PCT,
            "arms_humanoid",
            "arms_hanging",
            "arms_foreleg",
            "arms_bird",
            "arms_bat",
            "tail_none",
            "tail_short",
            "tail_hair",
            "tail_long",
            "stance_auto",
            "stance_left",
            "stance_right",
            // MIRROR FROM + FACE FORWARD — the other two source-shape knobs (697DEC55, 164AE2F3).
            "mirror_off",
            "mirror_left",
            "mirror_right",
            ui::FACE_FORWARD,
            ui::TAIL_BONES,
            ui::HEAD_ON,
            ui::PROBOSCIS_BONES,
            ui::QUADRUPED,
        ] {
            extent(&frame, id);
        }
        assert!(
            frame.rect(ui::HANG_CM).is_none(),
            "HANG lives with the SPLIT verb it feeds, not on Prep (T2's owed item 2)"
        );
        for (slot, _) in ui::RIG_SLOTS {
            let s = frame
                .surface(slot)
                .unwrap_or_else(|| panic!("{slot} reserved on Prep"));
            assert!(
                s.w > 100.0 && s.h > 100.0,
                "{slot} has extent: {}x{}",
                s.w,
                s.h
            );
        }

        b.tab = 2; // rig
        let frame = walk(&mut b);
        for (slot, _) in ui::RIG_SLOTS {
            assert!(frame.surface(slot).is_some(), "{slot} reserved on Rig");
        }
        for id in [
            ui::BAKE_SKIN,
            ui::BONE_RESET,
            "mode_translate",
            "mode_flip",
            ui::GIZMO_SNAP,
            ui::AUTO_DEPTH,
            ui::OFF_ROLL,
            ui::SOLO,
            ui::MIRROR_TO_TWIN,
            ui::FRAME_SEL,
            ui::BACKDROP_DARK,
            // THE REGIONS PANEL (spec 0A81088E T2) — the two steppers, the typed chain count,
            // the stiffness slider and the four verbs, every one with real extent on Rig.
            ui::REGION_TAG_PREV,
            ui::REGION_TAG_NEXT,
            ui::REGION_BONE_PREV,
            ui::REGION_BONE_NEXT,
            ui::REGION_CHAINS,
            ui::REGION_STIFFNESS,
            ui::REGION_SPLIT,
            ui::REGION_GROW,
            ui::REGION_CULLED,
            ui::REGION_REMOVE,
            // HANG (cm) rides with the panel whose SPLIT verb reads it, so it is here on Rig and
            // on Mount — the garment rail's only stop with a body under it (T2's owed item 2).
            ui::HANG_CM,
        ] {
            extent(&frame, id);
        }
        // The isolation chrome is the PANEL's own since H3, so the containment invariant
        // (380BDCC8) is measured where it can finally be honoured — inside the sub scene's
        // own tree, by `flicker_modelview`'s
        // `the_chrome_lands_inside_the_panel_with_real_extent`. What this bench still owes
        // is the SLOT, which the loop above measures.
        assert!(
            frame.rect(ui::DECIMATE).is_none(),
            "prep controls are dark on Rig"
        );
    }

    /// **THE RIG CELL SCROLLS INSTEAD OF RESOLVING ITS TAIL OFF-SCREEN** (Aaron
    /// 2026-09-15: *"some of the panels now vertical overflow the screen, these panels
    /// need to be able to scroll for content overflow"*). The Rig step stacks the marker
    /// rail, the bone list, four offset sliders, the gizmo modes, nine checkboxes and the
    /// verb rows into one column beside the 320px Regions panel — at a SHORT window that
    /// column is taller than its pane, and the twice-burned lesson (93B5000F) is that a
    /// tree which passes every shape gate can still put its content where no one can
    /// reach it. So: the last control is placed BELOW the fold, a wheel tick over the
    /// pane brings it up, and it keeps real extent throughout — scrolled, never squashed
    /// and never abandoned off-screen.
    #[test]
    fn a_short_window_scrolls_the_rig_column_rather_than_losing_it() {
        let mut b = bench();
        b.tab = 2; // rig
        let screen = Vec2::new(1600.0, 560.0);
        let pane = Vec2::new(-1.0, -1.0);
        let frame = walk_on(&mut b, screen, pane, 0.0);
        let pane_r = frame
            .rect("ap_controls")
            .expect("the controls pane is placed");
        let fold = pane_r.pos.y + pane_r.size.y;
        let last = frame
            .rect(ui::BACKDROP_DARK)
            .expect("the column's last control is still placed");
        assert!(
            last.size.x > 24.0 && last.size.y > 12.0,
            "a squeezed column keeps every control's extent: {:?}",
            last.size
        );
        assert!(
            last.pos.y > fold,
            "the column overflows its pane at a short window: {} vs fold {fold}",
            last.pos.y
        );

        // A wheel tick over the OVERFLOWING column scrolls it — the offset is that
        // container's own, so nothing in this bench publishes or folds a scroll key.
        // (The pointer picks the region: the Regions panel below it is a box of its own.)
        let head = frame
            .rect(ui::MARKER_PLACE)
            .expect("the column's first control is placed");
        let inside = head.pos + head.size * 0.5;
        walk_on(&mut b, screen, inside, -4.0);
        let frame = walk_on(&mut b, screen, inside, 0.0);
        let moved = frame
            .rect(ui::BACKDROP_DARK)
            .expect("still placed after the scroll");
        assert!(
            moved.pos.y < last.pos.y,
            "the wheel moved the column up: {} -> {}",
            last.pos.y,
            moved.pos.y
        );
        assert_eq!(moved.size, last.size, "scrolling resizes nothing");
    }

    /// EVERY VIEW SURFACE PLAYS THE `model_view` SCENE (ruling EBDB3518: a nested surface is a
    /// COMPLETE scene; H3 is its first consumer). The seven slots the bench drives are the
    /// seven the tree authors, each naming the sub scene and the projection the roster
    /// expects — and NOTHING else: no `source` (the panel brings its own `stages.rig`), no
    /// children (its chrome is inside its own tree, where a composite cannot paint over it —
    /// incident 09E5A30F). The parent tree only PLACES it.
    #[test]
    fn the_view_surfaces_play_the_model_view_scene() {
        let json: serde_json::Value = serde_json::from_str(ui::SCENE).expect("scene parses");
        fn walk<'a>(n: &'a serde_json::Value, out: &mut Vec<&'a serde_json::Value>) {
            if n["component"].as_str() == Some("surface") {
                out.push(n);
            }
            if let Some(kids) = n["children"].as_array() {
                kids.iter().for_each(|kid| walk(kid, out));
            }
        }
        let mut surfaces = Vec::new();
        walk(&json["tree"], &mut surfaces);
        for (slot, projection) in ui::view_slots() {
            let node = surfaces
                .iter()
                .find(|n| n["id"].as_str() == Some(slot))
                .unwrap_or_else(|| panic!("the tree authors a `{slot}` surface"));
            assert_eq!(
                node["scene"].as_str(),
                Some(ui::VIEW_SCENE),
                "{slot} plays the panel scene"
            );
            assert_eq!(
                node["scene_projection"].as_str(),
                Some(projection),
                "{slot} authors the projection the roster drives it as"
            );
            assert!(
                node["source"].is_null(),
                "{slot} names no stage — the sub scene brings its own"
            );
            assert!(
                node["children"].is_null(),
                "{slot} carries no chrome children — the panel's chrome is its own tree's"
            );
        }
        // And the SEVEN are all of them: a view surface the roster does not know would never
        // be seated, updated or composited.
        let named: Vec<&str> = surfaces
            .iter()
            .filter(|n| n["scene"].as_str() == Some(ui::VIEW_SCENE))
            .filter_map(|n| n["id"].as_str())
            .collect();
        let roster: Vec<&str> = ui::view_slots().map(|(slot, _)| slot).collect();
        assert_eq!(named, roster, "the tree names exactly the roster's panels");
        // The stopgaps the sub-scene shape replaced (decision 41D4F63E / dead end 98C0B1DB) are
        // gone from the pair — a chrome row left behind would draw dark and unreachable.
        for needle in [
            "view_overlay",
            "top_limb",
            "side_cull",
            "front_cull_at",
            "rig_top",
        ] {
            assert!(
                !ui::SCENE.contains(needle),
                "assetpipeline.scene.json still carries `{needle}` from the parent-tree chrome"
            );
        }
    }

    /// THE GADGET'S MODES ARE AUTHORED: `arrange()` publishes one gate per mode for the open
    /// step, and only the Rig step — whose `BoneOffset` carries a translation, a roll, a scale
    /// and a mirrorable twin — allows any. Every other step publishes none, which is an inert
    /// gadget rather than a control that silently does nothing.
    #[test]
    fn the_script_publishes_the_gadget_modes_of_the_open_step() {
        let mut b = bench();
        b.tab = 2; // rig
        let (_, model) = b.publish();
        for (gate, name) in ui::GADGET_MODE_GATES {
            assert!(model.is_on(gate), "the Rig step allows {name}");
        }
        for tab in [0, 1, 3, 4, 5] {
            b.tab = tab;
            let step = b.step();
            let (_, model) = b.publish();
            for (gate, name) in ui::GADGET_MODE_GATES {
                assert!(!model.is_on(gate), "{name} is gated off on {}", step.name());
            }
        }
        // And the names the gate publishes ARE the radios' values, so the two cannot drift.
        for ((_, gate), radio) in ui::GADGET_MODE_GATES.iter().zip(ui::GIZMO_VALUES) {
            assert_eq!(*gate, radio);
        }
    }

    /// The preview step reserves the bake view and nothing else; the animation workflow's
    /// clip step reserves the two variant views.
    #[test]
    fn the_preview_and_clip_steps_reserve_their_own_views() {
        let mut b = bench();
        b.tab = 3; // preview
        let frame = walk(&mut b);
        let (bake, _) = ui::BAKE_SLOT;
        let s = frame
            .surface(bake)
            .expect("the bake view is reserved on Preview");
        assert!(s.w > 100.0 && s.h > 100.0);
        for (slot, _) in ui::RIG_SLOTS {
            assert!(frame.surface(slot).is_none(), "{slot} is dark on Preview");
        }
        b.wf = Workflow::Animation;
        b.tab = 1; // clip
        let frame = walk(&mut b);
        for (slot, _) in ui::CLIP_SLOTS {
            let s = frame
                .surface(slot)
                .unwrap_or_else(|| panic!("{slot} reserved on Clip"));
            assert!(s.w > 100.0 && s.h > 100.0, "{slot} has extent");
        }
        assert!(
            frame.surface(bake).is_none(),
            "the bake view is dark on Clip"
        );
    }

    /// The script owns the flow's "what happens after": a `loaded` signal moves the rail to
    /// the first working stop, `next_piece` sends it home.
    #[test]
    fn the_script_answers_the_scene_signals_with_a_stop() {
        let mut b = bench();
        let mut sig = ValueMap::new();
        sig.set(ui::SIG_LOADED, true);
        sig.set(ui::WF_BIND, Workflow::Character.name());
        b.react(&sig);
        assert_eq!(b.step(), Step::Prep, "loaded → the first working stop");
        b.tab = 5;
        let mut sig = ValueMap::new();
        sig.set(ui::SIG_NEXT_PIECE, true);
        b.react(&sig);
        assert_eq!(b.step(), Step::Source, "next piece → home");
    }

    /// The rail's bound index moves the bench; a tab past the rail clamps to its last stop.
    #[test]
    fn the_rail_index_moves_the_step() {
        let mut b = bench();
        let mut r = ValueMap::new();
        r.set(ui::TAB_BIND, 1.0);
        b.apply_results(&r);
        assert_eq!(b.step(), Step::Prep);
        r.set(ui::TAB_BIND, 99.0);
        b.apply_results(&r);
        assert_eq!(b.step(), Step::Review);
        // Back at the first stop with nothing loaded asks nothing.
        b.tab = 0;
        let mut r = ValueMap::new();
        r.set(ui::STEP_PREV, true);
        b.apply_results(&r);
        assert!(!b.ask_discard);
    }

    /// THE UNSAVED-WORK PROMPT IS THE SHARED MODAL: the bench ARMS it (the dispatcher
    /// returns no transition, so `update` pushes it, exactly as it pushes the pause
    /// overlay) and reads its answer back through the kernel's `modal_closed` hook, into
    /// the SAME dispatcher a click feeds. Replaces the inline `ap_discard` subtree and
    /// the `discard_open` gate that used to swallow every other result while it was up.
    #[test]
    fn the_unsaved_prompt_arms_the_shared_modal_and_its_answer_comes_back() {
        // The ARM is what `update` turns into the push. Its guard (`dirty()`) needs a
        // loaded source — a content fixture — so its negative half is pinned in
        // `the_rail_index_moves_the_step` and this drives the armed state directly.
        let mut b = bench();
        b.ask_discard = true;

        // KEEP EDITING: the answer arrives through the kernel hook and changes nothing.
        b.tab = 2;
        b.modal_closed("choice_dialog", ui::DISCARD_NO, None);
        assert_eq!(
            b.tab, 2,
            "keeping the work leaves the bench exactly as it was"
        );

        // DISCARD: the same channel, and the bench is reset to the first stop.
        b.scrolls.insert(ui::ROWS_BONES, 12.0);
        b.modal_closed("choice_dialog", ui::DISCARD_YES, None);
        assert_eq!(b.tab, 0, "discarding sends the rail home");
        assert!(!b.dirty(), "and the document is fresh");
        assert!(
            b.scrolls.is_empty(),
            "and every list starts at the top again"
        );
    }

    /// THE INLINE MODAL IS GONE: no `ap_discard` subtree, no `screens.confirm` block and
    /// no `shown_discard` slice survive in the shipped pair — a migration that left the
    /// old copy behind would still render it, dark and unreachable, forever.
    #[test]
    fn no_inline_discard_modal_is_left_in_the_scene_pair() {
        for needle in ["ap_discard", "shown_discard", "screens.confirm"] {
            assert!(
                !ui::SCENE.contains(needle),
                "assetpipeline.scene.json still carries `{needle}` from the inline modal"
            );
        }
        let lua = include_str!("../../../../content/sensorium/scripts/assetpipeline.lua");
        assert!(
            !lua.contains("shown_discard") && !lua.contains("discard_open"),
            "assetpipeline.lua still lights the retired inline modal's slice"
        );
    }

    /// The workflow rails are exclusive: exactly one is lit, and it is the open workflow's.
    #[test]
    fn arrange_lights_the_open_workflows_rail() {
        let mut b = bench();
        b.wf = Workflow::Animation;
        b.tab = 1;
        let (_, model) = b.publish();
        assert!(model.is_on("shown_wf_animation"));
        assert!(!model.is_on("shown_wf_character"));
        assert!(model.is_on("shown_t_clip"));
        assert!(model.is_on("shown_view_clip"));
        assert!(!model.is_on("shown_view_quad"));
        assert_eq!(model.text(ui::STEP_TITLE), Some(Step::Clip.title()));
    }

    /// DEVELOPMENT-TIER GATES (Aaron 2026-09-05, ruling 977B4D38): the hard-coded handoff
    /// conditions of a refactor — tests that read this crate's own source and assert a
    /// transition holds. `cargo test -- --skip gates::` is the production tier (every OS);
    /// `cargo test -- gates::` runs only these (one OS in CI). A gate names the transition
    /// it enforces and is deleted when that transition closes.
    mod gates {
        /// THE OS DIALOG IS THE PICKER, and it is reached through ONE seam. `rfd` may be
        /// named only by `Document::pick_folder` in `services.rs` (and by the manifest that
        /// pulls it in): a second `FileDialog` anywhere in the bench would be a second door
        /// with its own start directory and its own title (rule 98232A50 — one path, no
        /// caller left on another). The needle is assembled rather than written, so this
        /// gate does not trip over its own text.
        #[test]
        fn the_os_dialog_is_reached_through_the_one_pick_seam() {
            let needle = ["r", "fd", "::"].concat();
            assert_eq!(
                include_str!("services.rs").matches(&needle).count(),
                1,
                "`services.rs` names the native-dialog crate exactly once — inside \
                 `Document::pick_folder`, the bench's ONE dialog seam"
            );
            for (what, src) in [
                ("scene.rs", include_str!("scene.rs")),
                ("compose.rs", include_str!("compose.rs")),
                ("ui.rs", include_str!("ui.rs")),
            ] {
                assert!(
                    !src.contains(&needle),
                    "{what} reaches for the native dialog directly — it goes through \
                     `Document::pick_folder` or it is a second door"
                );
            }
        }
    }
}
