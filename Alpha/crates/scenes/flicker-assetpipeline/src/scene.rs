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

use flicker::render::{FrameGraph, Renderer, TextureHandle};
use flicker::scene::{Scene, SceneInput, Transition};
use flicker::script::{HudCommand, ScriptHost, UiNode, ValueMap};
use flicker::ui::{
    instantiate_rows, render_hud, run_ui, strings, Row, SceneDef, UiInput, UiIntents, UiState,
    WalkerHandler,
};
use flicker_content::{AssetClass, PropKind};
use flicker_input_core::{AbstractControls, GamepadConfig, InputContext, InputMap, InputState};
use flicker_input_router::{InputHandler, Router};
use flicker_rigview::gadget::modes_from_names;
use flicker_rigview::{GadgetStyle, Projection, RigView};
use flicker_shell::{ModalParams, PauseScene, SharedModal, Theme};
use glam::{Mat4, Vec3};

use crate::compose::{self, Show};
use crate::gizmo::{Gizmo, GizmoUi};
use crate::meshes::ViewMeshes;
use crate::services::{
    class_label, BoneOffset, Document, MapState, WF_ANIMATION, WF_CHARACTER, WF_PROP,
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
    show: [bool; 4],
    /// The gadget's handle colours, resolved from the theme once (they never change).
    gadget_style: GadgetStyle,
    /// The four view panels (perspective, top, left, front), each seated from its own
    /// `surface` node and lit by its own stage.
    rig: Vec<RigView>,
    /// The preview step's bake view and the clip step's two variant views — single
    /// perspective panels, seated from their own surfaces while their slice is lit.
    extra: Vec<RigView>,
    /// Each panel's current framing; re-framed only when it changes.
    framed: Vec<(Vec3, f32)>,
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
        let rig = ui::RIG_SLOTS
            .iter()
            .zip(Projection::ALL)
            .map(|((_, stage), projection)| {
                RigView::new(stage, &ui_styles, projection).in_panel(ui::VIEW_PANE)
            })
            .collect();
        let extra = std::iter::once(ui::BAKE_SLOT)
            .chain(ui::CLIP_SLOTS)
            .map(|(_, stage)| {
                RigView::new(stage, &ui_styles, Projection::Perspective).in_panel(ui::VIEW_PANE)
            })
            .collect();
        Self {
            rig,
            extra,
            framed: vec![(Vec3::ZERO, 0.0); ui::RIG_SLOTS.len() + 1 + ui::CLIP_SLOTS.len()],
            meshes: ViewMeshes::new(),
            gadget_style: compose::gadget_style(&ui_styles),
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
            show: [true, true, false, false],
        }
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
        m.set(
            ui::PICK_SEL,
            self.doc.selected_candidate().unwrap_or_default(),
        );

        // Prep.
        m.set(ui::STATURE, f64::from(self.doc.stature_cm));
        m.set(ui::DECIMATE, self.doc.decimate_target.clone());
        m.set(
            ui::PREP_HEIGHT,
            Document::height_readout(self.doc.stature_cm),
        );
        m.set(ui::PREP_STATUS, self.doc.prep_status());

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
        m.set(ui::MIRROR, self.doc.mirror_joints);
        for (k, v) in ui::SHOW.iter().zip(self.show) {
            m.set(*k, v);
        }
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

        // Prep: the stature dial commits on release, the target field on submit.
        if let Some(v) = r.number(ui::STATURE) {
            let cm = v as f32;
            if cm != self.doc.stature_cm {
                self.doc.stature_cm = cm;
                self.doc.rebuild_prepped_model();
            }
        }
        if let Some(t) = r.text(ui::DECIMATE) {
            if t != self.doc.decimate_target {
                self.doc.decimate_target = t.to_string();
            }
        }
        if r.is_on(ui::DECIMATE_APPLY) {
            self.doc.apply_decimate_target();
        }
        if r.is_on(ui::DECIMATE_RESET) {
            self.doc.reset_decimate_target();
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
        if let Some(v) = r.get(ui::MIRROR).and_then(as_bool) {
            self.doc.mirror_joints = v;
        }
        for (i, k) in ui::SHOW.iter().enumerate() {
            if let Some(v) = r.get(k).and_then(as_bool) {
                self.show[i] = v;
            }
        }
        if r.is_on(ui::BAKE_SKIN) {
            self.doc.bake_skin_now();
        }
        if r.is_on(ui::BONE_RESET) {
            // "Reset bone" is zeroing the authored correction (the posed skeleton derives).
            self.doc.set_selected_offset(BoneOffset::default());
        }

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

    /// The decimate-target field owns the keyboard while its session is open.
    fn input_context(&self) -> Option<InputContext> {
        self.ui_state
            .text_entry()
            .then_some(InputContext::TextEntry)
    }

    fn enter(&mut self, renderer: &mut Renderer) {
        renderer.clear_color = [0.02, 0.03, 0.05, 1.0];
        self.meshes.enter(renderer);
        let theme = Theme::build(renderer);
        let entries = theme.lua_textures();
        self.textures = entries.iter().map(|(_, h)| *h).collect();
        self.theme = Some(theme);
    }

    fn update(
        &mut self,
        dt: Duration,
        input: &InputState,
        signals: &mut SceneInput,
        renderer: &Renderer,
    ) -> Transition {
        if let Some((_map, look, _gp)) = flicker_shell::take_pending_input() {
            for v in self.rig.iter_mut().chain(&mut self.extra) {
                v.set_controls(look);
            }
        }

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
        // The walker RESERVED the view panels' rects; seat each panel from its own slot
        // (a dark step reserved nothing, so its panel seats `None` and costs nothing).
        let slots = ui::RIG_SLOTS
            .into_iter()
            .chain(std::iter::once(ui::BAKE_SLOT))
            .chain(ui::CLIP_SLOTS);
        let mut pointers = Vec::with_capacity(self.rig.len() + self.extra.len());
        for (v, (slot, _)) in self.rig.iter_mut().chain(&mut self.extra).zip(slots) {
            v.seat(frame.surface(slot));
            pointers.push(frame.surface_pointer(slot).cloned());
        }
        self.hud_commands = frame.commands;

        let mut walker = WalkerHandler::hud(&mut self.ui_state, over_hud)
            .with_nav(&tree, &model)
            .with_intents(&self.ui_intents);
        {
            // The perspective panel sits BELOW the walker: navigation is decided first,
            // and what is left of the look/zoom signals is the camera's while the view
            // pane holds the cursor.
            let mut chain: [&mut dyn InputHandler; 2] = [&mut walker, &mut self.rig[0]];
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

        // The gizmo reads the four rig panels' pointer samples on the Rig step; the panel
        // whose pointer it consumed keeps its camera still this frame.
        let step = self.step();
        let show = Show {
            skeleton: self.show[0],
            base: self.show[1],
            collision: self.show[2],
            wireframe: self.show[3],
        };
        let rig_composed = compose::rig_lines(&self.doc, show, step, self.meshes.base());
        let gizmo_active = step == Step::Rig && self.doc.bone_sel().is_some();
        let gizmo_owned = self.gizmo_state.interact(
            &mut self.doc,
            &self.rig,
            &pointers[..self.rig.len()],
            gizmo_active,
            rig_composed.framing.radius,
        );
        // Re-compose after a drag moved a joint, so the overlay follows the pointer.
        let rig_composed = if gizmo_owned.is_some() {
            compose::rig_lines(&self.doc, show, step, self.meshes.base())
        } else {
            rig_composed
        };
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

        let look = RigView::look_from(|s| signals.axis(s, input));
        let focused = self.ui_state.focused_pane().map(str::to_string);
        let rig_len = self.rig.len();
        for (i, v) in self.rig.iter_mut().chain(&mut self.extra).enumerate() {
            let composed = if i < rig_len {
                let mut c = if v.projection() == Projection::Perspective {
                    rig_composed.clone()
                } else {
                    rig_composed.without_ground()
                };
                // The handles are PER PANEL — an orthographic view hides the axis it looks
                // along, and each handle wears its own Aim → Locked → Modify colour.
                if gizmo_active {
                    c.overlay.extend(
                        self.gizmo_state
                            .handle_lines(v.projection(), &self.gadget_style),
                    );
                }
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
            let frame_key = (composed.framing.centre, composed.framing.radius);
            if self.framed[i] != frame_key {
                self.framed[i] = frame_key;
                v.set_frame(composed.framing.centre, composed.framing.radius);
            }
            v.set_lines(composed.lines);
            v.set_overlay(composed.overlay);
            // The pad's look belongs to the perspective rig panel; the others take the
            // pointer only — and a panel the gizmo is dragging on takes neither.
            let pad = if i == 0 { look } else { (0.0, 0.0, 0.0) };
            let pointer = if gizmo_owned == Some(i) {
                None
            } else {
                pointers[i].as_ref()
            };
            v.update(dtf, pointer, pad, focused.as_deref());
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
        for v in self.rig.iter_mut().chain(&mut self.extra) {
            v.free(renderer);
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
        };
        if step == Step::Preview {
            self.meshes.bake(&mut self.doc, renderer);
        } else {
            self.meshes.release_bake(renderer);
        }
        let draws = compose::rig_draws(&self.doc, &mut self.meshes, renderer, step, show);
        for v in &mut self.rig {
            v.set_draws(draws.clone());
        }
        let bake_draw = self
            .meshes
            .bake_ref()
            .filter(|_| !self.bake_palette.is_empty())
            .map(|bp| bp.draw(self.bake_palette.clone()));
        self.extra[0].set_draws(bake_draw.into_iter().collect());

        let Self {
            rig,
            extra,
            hud_commands,
            textures,
            ..
        } = self;
        let layer = fg.base_layer();
        for v in rig.iter_mut().chain(extra.iter_mut()) {
            v.render(renderer, fg, layer);
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
            ui::OFF_ROLL,
        ] {
            extent(&frame, id);
        }
        assert!(
            frame.rect(ui::DECIMATE).is_none(),
            "prep controls are dark on Rig"
        );
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
}
