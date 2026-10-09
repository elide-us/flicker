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

/// The view pane — the `tab_group` whose cursor hands the panels the look signals
/// and decides which of them may hear a discrete event (the ruling's "input by intent";
/// the four rig panels share it). The facts and controls panes are the walker's alone.
pub const VIEW_PANE: &str = "ap_view";

/// The SUB SCENE every view surface plays — `scenes/model_view.scene.json`
/// (ruling EBDB3518: a nested surface is a COMPLETE scene). The bench places the slot and
/// hands it context; the panel's picture, its corner label / flip, its isolate row and its
/// own `rig` stage are the sub scene's, drawn inside the slot's own target.
pub const VIEW_SCENE: &str = "model_view";

/// The four rig-view surfaces (slot id, authored `scene_projection`), in the grid's order:
/// perspective, top, side, front. The projection is the HOST's half of the contract — the
/// gate `the_view_surfaces_play_the_model_view_scene` proves the tree authors these very
/// names on these very slots.
pub const RIG_SLOTS: [(&str, &str); 4] = [
    ("ap_view_persp", "persp"),
    ("ap_view_top", "top"),
    ("ap_view_side", "side"),
    ("ap_view_front", "front"),
];
/// The preview step's single bake view.
pub const BAKE_SLOT: (&str, &str) = ("ap_view_bake", "persp");
/// The clip step's two variant views (root motion, in place).
pub const CLIP_SLOTS: [(&str, &str); 2] = [("ap_view_root", "persp"), ("ap_view_place", "persp")];

/// Every view slot the bench hosts, in the order `Clayworks` builds and drives them: the
/// four quad panels, then the bake view, then the two clip views.
pub fn view_slots() -> impl Iterator<Item = (&'static str, &'static str)> {
    RIG_SLOTS
        .into_iter()
        .chain(std::iter::once(BAKE_SLOT))
        .chain(CLIP_SLOTS)
}

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
pub const IMPORT_CREATURE: &str = "import_creature";
pub const DECIMATE_RESET: &str = "prep_decimate_reset";
pub const DECIMATE_APPLY: &str = "prep_decimate_apply";
/// The height's own APPLY — the typed number resizes the body (and its skeleton, if rigged).
pub const STATURE_APPLY: &str = "prep_stature_apply";
/// FACING — turn the raw mesh a quarter-turn about the vertical so a broadside source faces the
/// rig (`Document::turn_facing`); its readout is the turn in degrees.
pub const FACING_TURN: &str = "prep_facing_turn";
pub const SKELETON_PREV: &str = "skeleton_prev";
pub const SKELETON_NEXT: &str = "skeleton_next";
/// The Prep step's MODULE controls (P2c S4, ruling 0C796096): each edits the document's recipe
/// from the picked preset as a starting point. Radios publish their value text on the bind.
pub const LEGS: &str = "legs";
pub const LEG_VALUES: [&str; 5] = ["flat", "heeled", "toewalker", "hooves", "bird"];
pub const HEEL_PCT: &str = "heel_pct";
pub const ARMS: &str = "arms";
pub const ARM_VALUES: [&str; 5] = ["humanoid", "hanging", "foreleg", "bird", "bat"];
/// SQUARE FROM — which side the bake-time stance normaliser mirrors a MID-STRIDE body from
/// (Aaron's ruling FEFDA2B2). Auto takes the planted limb of each pair; Left/Right name the
/// source outright. Rides with the body beside the facing knob and is read by the ONE bake path.
pub const STANCE: &str = "stance_source";
pub const STANCE_VALUES: [&str; 3] = ["auto", "left", "right"];
/// MIRROR FROM — which half of a LOPSIDED source the mesh mirror keeps and reflects (direction
/// 697DEC55, side ruling FEFDA2B2). "Off" is the default: a mirror destroys everything one-sided
/// that is not tagged. Rides with the body beside [`STANCE`] and is applied in Prep, BEFORE the
/// skeleton is fitted, so every twin-joint assumption the fit makes holds on the mirrored body.
pub const MIRROR_KEEP: &str = "mirror_keep";
pub const MIRROR_KEEP_VALUES: [&str; 3] = ["off", "left", "right"];
/// FACE FORWARD — un-turn a head bound off the canon forward (164AE2F3). ON by default (A79A6131:
/// turned heads are common across the generated sources): the box is the OPT-OUT. Read by the ONE
/// bake path right after the stance normaliser, so the Preview page plays exactly what Commit
/// writes.
pub const FACE_FORWARD: &str = "face_forward";
pub const TAIL: &str = "tail";
pub const TAIL_VALUES: [&str; 4] = ["none", "short", "hair", "long"];
pub const TAIL_BONES: &str = "tail_bones";
/// HANG (cm) — how far off the body a garment vertex must stand to read as cloth, the one
/// measurement the region split makes (spec 0A81088E). Typed like the stature, rides with the
/// document like [`STANCE`], and opens at `flicker_content::DEFAULT_HANG_CM`.
///
/// It lives in the REGIONS panel, not on Prep: the knob belongs to the SPLIT verb it feeds, and
/// the Prop/garment rail (Source → Mount → Review) never reaches a Prep page, so a garment's split
/// ran at the default for as long as the field sat there (T2's owed item 2). The Regions panel
/// shows on Rig AND Mount, so ONE authored field serves both rails and one document value.
pub const HANG_CM: &str = "hang_cm";
pub const HEAD_ON: &str = "head_on";
/// The PROBOSCIS — how many bones the chain off the head's front has, 0 for none (ruling
/// 7881216F: the elephant's trunk curls and reaches). A digits field beside the head's toggle.
pub const PROBOSCIS_BONES: &str = "proboscis_bones";
pub const QUADRUPED: &str = "quadruped";
pub const BAKE_SKIN: &str = "bake_skin";
pub const BONE_RESET: &str = "bone_reset";
/// THE REGION TAGGER's three verbs and its per-row Remove (spec 0A81088E T2). SPLIT runs the
/// garment split against the fitting body at the Prep page's HANG; GROW arms the next perspective
/// click for a thin-part grow; CULLED takes whatever the ortho panels' cut planes hide.
pub const REGION_SPLIT: &str = "region_split";
pub const REGION_GROW: &str = "region_grow";
pub const REGION_CULLED: &str = "region_culled";
pub const REGION_REMOVE: &str = "region_remove";
/// The two STEPPERS over the selected region's row — the tag and the anchor bone. Same ‹ › shape
/// the skeleton pick uses (5DE94A49), each step one place along its own list.
pub const REGION_TAG_PREV: &str = "region_tag_prev";
pub const REGION_TAG_NEXT: &str = "region_tag_next";
pub const REGION_BONE_PREV: &str = "region_bone_prev";
pub const REGION_BONE_NEXT: &str = "region_bone_next";
/// THE MARKERS RAIL's four buttons (spec FF40E825; Infer is a BUTTON by ruling 42AB9BA8, never
/// automatic — there is no undo in the Rig step). `MARKER_NEXT` is the ACCEPT button
/// (`$ap_marker_accept`, incident 9715303C): it goes to the next joint that still wants a human,
/// and it is the ONLY way on — a drag's release never moves the rail. SKIP and BACK step the list
/// one place either way; INFER derives every joint the human has not placed. None of them moves a
/// joint by itself — ACCEPT/SKIP/BACK only select and frame, and the free ortho drag underneath is
/// what places.
pub const MARKER_NEXT: &str = "marker_next";
pub const MARKER_BACK: &str = "marker_back";
pub const MARKER_SKIP: &str = "marker_skip";
pub const MARKER_INFER: &str = "marker_infer";
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
/// AUTO DEPTH: an orthographic release resolves the joint's HIDDEN axis out of the mesh
/// (ruling F9F728CA). Default ON; OFF leaves the depth exactly where the hand left it.
pub const AUTO_DEPTH: &str = "auto_depth";
pub const MIRROR: &str = "mirror";
/// MIRROR →: put the selected subtree's twins at its reflection (`Document::mirror_to_twin`).
pub const MIRROR_TO_TWIN: &str = "mirror_to_twin";
/// FRAME: pan and zoom every panel onto the selected subtree.
pub const FRAME_SEL: &str = "frame_sel";
/// SOLO: the three orthographic panels draw the selected subtree only; the picker dims the rest.
/// The bench's own Rig control — the PER-PANEL half of the isolation (LIMB, CULL, CULL AT) is
/// the panel's own chrome now and is read back through `ModelView::isolate`.
pub const SOLO: &str = "solo";
/// The REGIONS list's pick (a region's name), and the two per-row knobs that are not steppers:
/// the chain count as typed (0 = rigid to the anchor, like the decimate target) and the standard
/// stiffness slider, which commits on release (B694F6B1).
pub const REGION_SEL: &str = "region_sel";
pub const REGION_CHAINS: &str = "region_chains";
pub const REGION_STIFFNESS: &str = "region_stiffness";
/// DISPLAY: clear every view panel to black instead of the stages' grey (dark models).
pub const BACKDROP_DARK: &str = "backdrop_dark";
pub const SHOW: [&str; 5] = [
    "show_skeleton",
    "show_base",
    "show_collision",
    "show_wireframe",
    "show_pbr",
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
pub const ROWS_REGIONS: &str = "ap_regions";
/// Every `rows_from` source the tree authors, with its list's scroll bind.
pub const ROW_SOURCES: [(&str, &str); 6] = [
    (ROWS_PICKS, "ap_picks_scroll"),
    (ROWS_BONES, "ap_bones_scroll"),
    (ROWS_SOCKETS, "ap_sockets_scroll"),
    (ROWS_CLIPS, "ap_clips_scroll"),
    (ROWS_ATTACH, "ap_attach_scroll"),
    (ROWS_REGIONS, "ap_regions_scroll"),
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
pub const FACING_READOUT: &str = "prep_facing";
/// The MARKERS RAIL's caption — the whole "Place: <joint>" sentence, resolved and formatted where
/// every other readout is. The JOINT NAME is data (a bone is never translated); the sentence is
/// the stringtable's, so its word order is the translator's (D5ED9ACF).
pub const MARKER_PLACE: &str = "marker_place";
/// THE MATCH STATUS — the Rig cell's one read-only line about what the SHAPE GRAPH made of this
/// body (spec 04803E0C, S2 431D08DF): how many of the recipe's modules found a partner in the
/// mesh, and which ones the rail is therefore walking. Pre-formatted like every other readout;
/// the sentence is the stringtable's and the counts and joint names go in as data. Empty when
/// there is no match to report.
pub const MARKER_MATCH: &str = "marker_match";
pub const SKELETON_NAME: &str = "skeleton_name";
pub const SKELETON_SUMMARY: &str = "skeleton_summary";
pub const PREVIEW_STATUS: &str = "preview_status";
/// The selected region's TAG and ANCHOR BONE, as the two steppers' readouts — resolved names, not
/// numbers, exactly as the skeleton stepper publishes its pick.
pub const REGION_TAG: &str = "region_tag";
pub const REGION_BONE: &str = "region_bone";
/// The REGIONS panel's gate — a BENCH-published `visible_bind` (the shape `has_picks` below uses),
/// not one of `arrange()`'s step slices: the panel belongs to TWO stops, the Rig step where a
/// character's hair and tail are tagged and the Mount step where a garment is placed on the body
/// and split. One authored block, one flag, rather than the same block in two slices.
pub const SHOWN_REGIONS: &str = "shown_regions";
pub const HAS_PICKS: &str = "has_picks";
/// A source folder is open — the script shows the Source step's preview on it.
pub const HAS_SOURCE: &str = "has_source";
pub const HAS_COMMITTED: &str = "has_committed";
/// The Review page's line about the last Commit — written where, or why not.
pub const COMMIT_NOTE: &str = "commit_note";
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
    /// A non-humanoid body: a character on a non-humanoid recipe (Aaron 2026-09-08, ruling
    /// 59B994A4 — the Creature workflow gained Rig · Preview · Attach). The SAME six-step rail as
    /// [`Self::Character`] (Source → Prep → Rig → Preview → Attach → Review): the Rig step
    /// composes the picked skeleton (a quadruped by default) onto the mesh, Preview walks it, and
    /// Commit writes a real `flicker.rig`. A creature committed WITHOUT reaching Rig still ships a
    /// boneless static bake (the "look now, rig later" fallback in `commit_to`), which is what a
    /// truncated rail used to force. The rail's tabs MUST match [`Self::steps`] — the
    /// `every_workflow_rail_matches_its_steps` gate holds the two together.
    Creature,
}

impl Workflow {
    #[cfg(test)]
    pub const ALL: [Workflow; 4] = [Self::Character, Self::Prop, Self::Animation, Self::Creature];

    /// The name the script reads (`Model.wf`) and the rail's `shown_wf_<name>` gate.
    pub fn name(self) -> &'static str {
        match self {
            Self::Character => "character",
            Self::Prop => "prop",
            Self::Animation => "animation",
            Self::Creature => "creature",
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
            // A creature is a character on a non-humanoid recipe (2026-09-08): the same rail.
            Self::Creature => &[
                Step::Source,
                Step::Prep,
                Step::Rig,
                Step::Preview,
                Step::Attach,
                Step::Review,
            ],
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

    /// AUTO DEPTH rides BESIDE the snap toggle in the Rig controls (ruling F9F728CA) — the same
    /// checkbox shape, the same pane, its own stringtable label, and the very next node: the tree
    /// is where a control's existence and its place are declared (491BD9BB).
    #[test]
    fn the_rig_controls_carry_auto_depth_beside_gizmo_snap() {
        let json: serde_json::Value = serde_json::from_str(SCENE).expect("scene parses");
        fn walk<'a>(n: &'a serde_json::Value, out: &mut Option<&'a serde_json::Value>) {
            if let Some(kids) = n.get("children").and_then(|v| v.as_array()) {
                if let Some(i) = kids.iter().position(|k| k["id"] == GIZMO_SNAP) {
                    *out = kids.get(i + 1);
                }
                kids.iter().for_each(|k| walk(k, out));
            }
        }
        let mut next = None;
        walk(&json["tree"], &mut next);
        let next = next.unwrap_or_else(|| panic!("`{GIZMO_SNAP}` is in the tree with a sibling"));
        assert_eq!(next["id"].as_str(), Some(AUTO_DEPTH), "the very next node");
        assert_eq!(next["bind"].as_str(), Some(AUTO_DEPTH));
        assert_eq!(next["component"].as_str(), Some("checkbox"), "same shape");
        assert_eq!(next["tab_group"].as_str(), Some("ap_controls"));
        assert_eq!(
            next["label"].as_str(),
            Some("$ap_auto_depth"),
            "every UI string comes through the stringtable (D5ED9ACF)"
        );
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
