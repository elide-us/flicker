//! **The bench's roster, as data.**
//!
//! The stable ids / binds / actions / row sources shared by the static component tree
//! (`assetpipeline.scene.json`), the Model and the ONE dispatcher, so the three cannot
//! drift apart — plus the workflow → step roster `assetpipeline.lua`'s `arrange()`
//! mirrors (the tab index names a step; the script lights that step's slice).
//!
//! This module BUILDS nothing: the surface is authored as data (a root `surface` →
//! `stack` → `paged_menu` with the page rail off, three step rails gated per workflow,
//! the facts / view / controls panes, the nav footer and the discard modal).

/// The authored scene, shipped with the crate (the manifest hands the parsed def to
/// [`scene`](crate::scene); the drift gates parse this copy).
#[cfg(test)]
pub const SCENE: &str =
    include_str!("../../../../content/sensorium/scenes/assetpipeline.scene.json");
/// The Lua orchestration layer — `arrange()` only.
pub const SCRIPT: &str = include_str!("../../../../content/sensorium/scripts/assetpipeline.lua");
pub const SCRIPT_NAME: &str = "assetpipeline.lua";

// ── Panes and surfaces ──────────────────────────────────────────────────────

/// The view pane — the `tab_group` whose cursor hands the perspective panel the look
/// signals. (The facts and controls panes are the walker's alone: the tree names them.)
pub const VIEW_PANE: &str = "ap_view";

/// The four rig-view surfaces (slot id, `stages.<source>`), in the grid's order:
/// perspective, top, side, front.
pub const RIG_SLOTS: [(&str, &str); 4] = [
    ("ap_view_persp", "rig_persp"),
    ("ap_view_top", "rig_top"),
    ("ap_view_side", "rig_side"),
    ("ap_view_front", "rig_front"),
];
/// The preview step's single bake view.
pub const BAKE_SLOT: (&str, &str) = ("ap_view_bake", "rig_bake");
/// The clip step's two variant views (root motion, in place).
pub const CLIP_SLOTS: [(&str, &str); 2] = [
    ("ap_view_root", "clip_root"),
    ("ap_view_place", "clip_place"),
];

// ── Selection ───────────────────────────────────────────────────────────────

/// Which workflow is open — a NAME the script reads (`Model.wf`).
pub const WF_BIND: &str = "wf";
/// The step rail's two-way bind: the selected step's index into the open workflow.
pub const TAB_BIND: &str = "tab";
/// The paged menu shows its tab rail only while this Model flag is on (Populous
/// publishes it per page); this bench's one page always has a step rail.
pub const TABS_SHOWN: &str = "paged_tabs_shown";

// ── Scene-level signals the script's `react()` answers ──────────────────────

/// A folder opened (`wf` names the workflow); the script says which stop comes next.
pub const SIG_LOADED: &str = "loaded";
/// The next piece of a multi-mesh folder started; the script sends the rail home.
pub const SIG_NEXT_PIECE: &str = "next_piece";

// ── Actions ─────────────────────────────────────────────────────────────────

pub const PAUSE_OPEN: &str = "pause_open";
/// The rail's back step: it steps the rail ITSELF; the scene reads it only to ask before
/// work is lost at the first stop. (`step_next` is the rail's alone.)
pub const STEP_PREV: &str = "step_prev";
pub const IMPORT_CHARACTER: &str = "import_character";
pub const IMPORT_ACCESSORY: &str = "import_accessory";
pub const IMPORT_PROP: &str = "import_prop";
pub const IMPORT_ANIMATION: &str = "import_animation";
pub const DECIMATE_RESET: &str = "prep_decimate_reset";
pub const DECIMATE_APPLY: &str = "prep_decimate_apply";
pub const BAKE_SKIN: &str = "bake_skin";
pub const BONE_RESET: &str = "bone_reset";
pub const NEXT_PIECE: &str = "next_piece";
pub const COMMIT: &str = "commit";
/// The two answers the SHARED `choice_dialog` modal carries back when Back leaves a
/// dirty first stop — the bench's own action names, handed to the modal as its options
/// and returned verbatim through `Scene::modal_closed` into the ONE dispatcher.
pub const DISCARD_YES: &str = "discard_yes";
pub const DISCARD_NO: &str = "discard_no";

// ── Two-way binds ───────────────────────────────────────────────────────────

pub const PREFER_STAGED: &str = "prefer_staged";
pub const AS_PROVIDED: &str = "as_provided";
pub const PICK_SEL: &str = "pick_sel";
pub const STATURE: &str = "stature_cm";
pub const DECIMATE: &str = "decimate_target";
pub const BONE_SEL: &str = "bone_sel";
pub const OFF: [&str; 3] = ["off_x", "off_y", "off_z"];
pub const OFF_ROLL: &str = "off_roll";
pub const GIZMO_MODE: &str = "gizmo_mode";
pub const GIZMO_SNAP: &str = "gizmo_snap";
pub const MIRROR: &str = "mirror";
pub const SHOW: [&str; 4] = [
    "show_skeleton",
    "show_base",
    "show_collision",
    "show_wireframe",
];
pub const RIG_PROGRESS: &str = "rig_progress";
pub const SOCK_SEL: &str = "sock_sel";
pub const FIT_OFFSET: [&str; 3] = ["fit_ox", "fit_oy", "fit_oz"];
pub const FIT_ROT: [&str; 3] = ["fit_rx", "fit_ry", "fit_rz"];
pub const FIT_SCALE_AXES: [&str; 3] = ["fit_sx", "fit_sy", "fit_sz"];
pub const FIT_SCALE: &str = "fit_scale";
pub const VARIANT_RM: &str = "variant_rm";
pub const VARIANT_IP: &str = "variant_ip";
pub const ATT_SEL: &str = "att_sel";
pub const ATT: [&str; 3] = ["att_x", "att_y", "att_z"];

/// The gizmo radios' values — the mode the rig view's handles edit in. These ARE the gadget's mode
/// NAMES (`flicker_rigview::modes_from_names` owns the spelling), so the radio a human presses and
/// the gate `assetpipeline.lua` publishes cannot drift into two vocabularies. The order is
/// `GizmoUi`'s discriminant order.
pub const GIZMO_VALUES: [&str; 4] = ["translate", "rotate", "scale", "flip"];

/// The per-step gadget gate `arrange()` publishes: one key per mode, ON when this step's document
/// has a consumer for it. `arrange()` marshals scalars keyed by component id, so the LIST of mode
/// names travels as four booleans and is re-assembled here — the same `{ on = … }` shape every
/// other slice gate in that script uses.
pub const GADGET_MODE_GATES: [(&str, &str); 4] = [
    ("gadget_translate", GIZMO_VALUES[0]),
    ("gadget_rotate", GIZMO_VALUES[1]),
    ("gadget_scale", GIZMO_VALUES[2]),
    ("gadget_flip", GIZMO_VALUES[3]),
];

// ── Data-driven rows ────────────────────────────────────────────────────────

pub const ROWS_PICKS: &str = "ap_picks";
pub const ROWS_BONES: &str = "ap_bones";
pub const ROWS_SOCKETS: &str = "ap_sockets";
pub const ROWS_CLIPS: &str = "ap_clips";
pub const ROWS_ATTACH: &str = "ap_attach";
/// Every `rows_from` source the tree authors, with its list's scroll bind.
pub const ROW_SOURCES: [(&str, &str); 5] = [
    (ROWS_PICKS, "ap_picks_scroll"),
    (ROWS_BONES, "ap_bones_scroll"),
    (ROWS_SOCKETS, "ap_sockets_scroll"),
    (ROWS_CLIPS, "ap_clips_scroll"),
    (ROWS_ATTACH, "ap_attach_scroll"),
];

// ── Readouts (pre-formatted text; a number never reaches a node) ────────────

pub const STEP_TITLE: &str = "step_title";
pub const STEP_HINT: &str = "step_hint";
pub const ASSET_NAME: &str = "asset_name";
pub const CLASS_LABEL: &str = "class_label";
pub const FACT_TRIS: &str = "fact_tris";
pub const FACT_VERTS: &str = "fact_verts";
pub const FACT_BONES: &str = "fact_bones";
pub const FACT_CLIPS: &str = "fact_clips";
pub const FACT_STATUS: &str = "fact_status";
pub const STATUS: &str = "ap_status";
pub const PREP_HEIGHT: &str = "prep_height";
pub const PREP_STATUS: &str = "prep_status";
pub const PREVIEW_STATUS: &str = "preview_status";
pub const HAS_PICKS: &str = "has_picks";
pub const HAS_COMMITTED: &str = "has_committed";
/// The review step's requirement rows: `req_<i>` (text) + `req_<i>_state` (badge token).
pub const REQ_ROWS: usize = 4;
pub fn req_bind(i: usize) -> String {
    format!("req_{i}")
}
pub fn req_state_bind(i: usize) -> String {
    format!("req_{i}_state")
}

// ── The workflow → step roster (mirrored by `assetpipeline.lua`'s STEPS) ────

/// The three linear workflows the Source step's import buttons open (ruling 2026-08-01:
/// a required branch is a SEPARATE definition, never a conditional in the spine).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Workflow {
    Character,
    Prop,
    Animation,
}

impl Workflow {
    #[cfg(test)]
    pub const ALL: [Workflow; 3] = [Self::Character, Self::Prop, Self::Animation];

    /// The name the script reads (`Model.wf`) and the rail's `shown_wf_<name>` gate.
    pub fn name(self) -> &'static str {
        match self {
            Self::Character => "character",
            Self::Prop => "prop",
            Self::Animation => "animation",
        }
    }

    /// The workflow's step rail, in the rail's authored order (the `tab` index indexes it).
    pub fn steps(self) -> &'static [Step] {
        match self {
            Self::Character => &[
                Step::Source,
                Step::Prep,
                Step::Rig,
                Step::Preview,
                Step::Attach,
                Step::Review,
            ],
            Self::Prop => &[Step::Source, Step::Mount, Step::Review],
            Self::Animation => &[Step::Source, Step::Clip, Step::Review],
        }
    }
}

/// One stop on a workflow's rail. The tree gates every stop's components on
/// `shown_t_<name>`; the script lights exactly one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    Source,
    Prep,
    Rig,
    Preview,
    Attach,
    Review,
    Mount,
    Clip,
}

impl Step {
    pub fn name(self) -> &'static str {
        match self {
            Self::Source => "source",
            Self::Prep => "prep",
            Self::Rig => "rig",
            Self::Preview => "preview",
            Self::Attach => "attach",
            Self::Review => "review",
            Self::Mount => "mount",
            Self::Clip => "clip",
        }
    }

    /// The step's title `$token` (the body's heading).
    pub fn title(self) -> &'static str {
        match self {
            Self::Source => "$ap_step_source",
            Self::Prep => "$wf_step_prep",
            Self::Rig => "$wf_step_rig",
            Self::Preview => "$wf_step_preview",
            Self::Attach => "$wf_step_attach",
            Self::Review => "$wf_step_review",
            Self::Mount => "$wf_step_mount",
            Self::Clip => "$wf_step_clip",
        }
    }

    /// The step's hint `$token` — what the user is meant to DO here.
    pub fn hint(self) -> &'static str {
        match self {
            Self::Source => "$ap_load_an_asset_folder_to_begin",
            Self::Prep => "$ap_prep_hint",
            Self::Rig => "$ap_map_the_source_skeleton_to_the_internal",
            Self::Preview => "$ap_preview_hint",
            Self::Attach => "$ap_position_hold_holster_and_belt_attach_po",
            Self::Review => "$ap_verify_engine_requirements_then_export",
            Self::Mount => "$ap_bind_the_piece_to_a_socket_then_place_it",
            Self::Clip => "$ap_preview_both_variants_pick_what_commit_k",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The names the tree owns outright (no scene code reads them): the rail's forward
    /// step and the three panes.
    const STEP_NEXT: &str = "step_next";
    const PANES: [&str; 3] = ["ap_facts", VIEW_PANE, "ap_controls"];

    fn squash(s: &str) -> String {
        s.split_whitespace().collect::<Vec<_>>().join(" ")
    }

    /// The Rust roster and the script's STEPS table are the same data twice — one
    /// authored for the dispatcher, one for `arrange()` — so a stop added to one and not
    /// the other is caught here, not in the window.
    #[test]
    fn the_script_mirrors_the_step_roster() {
        let flat = squash(SCRIPT);
        for wf in Workflow::ALL {
            let names: Vec<String> = wf
                .steps()
                .iter()
                .map(|s| format!("\"{}\"", s.name()))
                .collect();
            let line = squash(&format!("{} = {{ {} }}", wf.name(), names.join(", ")));
            assert!(
                flat.contains(&line),
                "assetpipeline.lua STEPS lacks `{line}`"
            );
        }
    }

    /// Every step gates a slice in the tree, every workflow gates a rail, and the row
    /// sources the tree names are exactly the roster's.
    #[test]
    fn the_tree_gates_every_step_rail_and_row_source() {
        let json: serde_json::Value = serde_json::from_str(SCENE).expect("scene parses");
        fn walk(n: &serde_json::Value, gates: &mut Vec<String>, sources: &mut Vec<String>) {
            if let Some(g) = n.get("visible_bind").and_then(|v| v.as_str()) {
                gates.push(g.to_string());
            }
            if let Some(s) = n.get("rows_from").and_then(|v| v.as_str()) {
                sources.push(s.to_string());
            }
            if let Some(kids) = n.get("children").and_then(|v| v.as_array()) {
                kids.iter().for_each(|k| walk(k, gates, sources));
            }
        }
        let (mut gates, mut sources) = (Vec::new(), Vec::new());
        walk(&json["tree"], &mut gates, &mut sources);
        for wf in Workflow::ALL {
            let gate = format!("shown_wf_{}", wf.name());
            assert!(gates.contains(&gate), "no rail gated on `{gate}`");
            for step in wf.steps() {
                let gate = format!("shown_t_{}", step.name());
                assert!(gates.contains(&gate), "no slice gated on `{gate}`");
            }
        }
        for s in &sources {
            assert!(
                ROW_SOURCES.iter().any(|(name, _)| name == s),
                "tree names row source `{s}` the roster lacks"
            );
        }
        for (name, _) in ROW_SOURCES {
            assert!(
                sources.contains(&name.to_string()),
                "roster source `{name}` is not in the tree"
            );
        }
    }

    /// The root declares the shoulder intents on the very names the step rails step
    /// themselves on, and every control belongs to one of the three panes.
    #[test]
    fn the_shoulders_step_the_rails_and_every_control_has_a_pane() {
        let json: serde_json::Value = serde_json::from_str(SCENE).expect("scene parses");
        let root = &json["tree"];
        assert_eq!(root["on_tab_next"].as_str(), Some(STEP_NEXT));
        assert_eq!(root["on_tab_prev"].as_str(), Some(STEP_PREV));
        assert_eq!(root["on_menu"].as_str(), Some(PAUSE_OPEN));
        fn walk(n: &serde_json::Value, rails: &mut usize, groups: &mut Vec<String>) {
            if n["component"].as_str() == Some("pill_toggle") {
                assert_eq!(
                    n["next_action"].as_str(),
                    Some(STEP_NEXT),
                    "rail {}",
                    n["id"]
                );
                assert_eq!(
                    n["prev_action"].as_str(),
                    Some(STEP_PREV),
                    "rail {}",
                    n["id"]
                );
                assert_eq!(n["bind"].as_str(), Some(TAB_BIND), "rail {}", n["id"]);
                *rails += 1;
            }
            if let Some(g) = n.get("tab_group").and_then(|v| v.as_str()) {
                groups.push(g.to_string());
            }
            if let Some(kids) = n.get("children").and_then(|v| v.as_array()) {
                kids.iter().for_each(|k| walk(k, rails, groups));
            }
        }
        let (mut rails, mut groups) = (0, Vec::new());
        walk(root, &mut rails, &mut groups);
        assert_eq!(rails, Workflow::ALL.len(), "one step rail per workflow");
        // The unsaved-work prompt is the SHARED `choice_dialog` modal now (pushed over
        // this scene, not authored into it), so every group left in this tree is a pane
        // or the footer — no modal exemption remains.
        for g in groups.iter().filter(|g| *g != "ap_footer") {
            assert!(PANES.contains(&g.as_str()), "control in unknown pane `{g}`");
        }
    }
}
