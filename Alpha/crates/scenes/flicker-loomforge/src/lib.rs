//! **Loomforge Bench** — the animation-authoring editor scene.
//!
//! A four-tab bench over a `flicker.pack`: State Machine · Pack Browser · Creature
//! Composer · TAE Editor. Unlike the retired `flicker-packeditor` (a read-only viewer),
//! this one WRITES the pack back — see [`EditorDoc::save`].
//!
//! # The scene is a PAIR (five-line architecture)
//!
//! `loomforge.scene.json` authors the chrome tree + this bench's style blocks;
//! `loomforge.lua` derives the presentation (page gates, tab/tool/filter/card/response
//! washes) from the RAW model this behaviour publishes; the Rust component kinds draw.
//! The named container cells (`lf_clip_rows` / `lf_pack_cards` / `lf_skel_rows`) are
//! REFILLED here at EVENT time (pack load / scroll / filter / scan) — the sablework
//! Rust-fills-the-container pattern — so the clip-row dolls keep their content-keyed
//! slot ids (the poster-cache key) and the rows stay drag sources.
//!
//! The scene owns no resolver and no bindings — the PUMP hands it resolved signals,
//! the walker consumes the screen's declared intents (`on_menu` / `on_tab_*` = page
//! cycling / `on_mode_*` = tool cycling), and both input channels land in the ONE
//! dispatch as result names. The node-graph canvas and TAE timeline stay scene-drawn
//! (free 2D positioning, edges, drop targets) inside the walker-reserved rtt rects
//! (`lf_canvas` / `lf_tae_strip` / `lf_tae_page_strip`).

mod canvas;
mod doc;
mod packs;
mod stage;
mod tae;

pub use doc::{EditorDoc, Tab, Tool};

use doc::{next_trigger, trigger_label, EdgeRef};
use flicker_skeletal::state::{EventKind, Response, Trigger};

use std::path::{Path, PathBuf};
use std::time::Duration;

use canvas::{CanvasArea, CardRect};
use flicker::render::{FrameGraph, Rect as StageRect, Renderer, TextureHandle, Vec2};
use flicker::scene::{Scene, SceneInput, Transition};
use flicker::script::{HudCommand, ScriptHost, UiNode, Value, ValueMap};
use flicker::ui::{
    render_hud, run_ui, strings, SceneDef, UiInput, UiIntents, UiState, WalkerHandler,
};
use flicker_input_core::{AbstractControls, GamepadConfig, InputMap, InputState};
use flicker_input_router::{InputHandler, Router};
use flicker_shell::{PauseScene, Theme};
use stage::{StageReq, StageRig};

/// The pair script — the scene's LOGIC half, by name (five-line architecture).
const LF_SCRIPT: &str = include_str!("../../../../content/sensorium/scripts/loomforge.lua");
/// The shipped scene file — the tests' copy of the authored tree (the runtime
/// receives the same file through the manifest `SceneDef`).
#[cfg(test)]
const LF_SCENE: &str = include_str!("../../../../content/sensorium/scenes/loomforge.scene.json");

/// The pack the bench opens with — the GOLEM baseline pack (seeded 2026-08-04 from the
/// retired PrismHumanBaseA exemplar): locomotion states over the shared retarget clips.
fn pack_path() -> PathBuf {
    flicker_core::roots::roots()
        .package()
        .join("characters/GolemBase_Low/GolemBase_Low.pack.json")
}
/// The base body rig, and the clip library the pack's state clips resolve against —
/// THE REFERENCE (GolemBase_Low) since the 2026-08-04 content sweep.
fn base_dir() -> PathBuf {
    flicker_core::roots::roots()
        .package()
        .join("characters/GolemBase_Low")
}
// The shared retarget library — the katanami-era per-character clip bundle was deleted
// with the example content (2026-08-04 audit); clips resolve by canonical bone name.
fn clips_dir() -> PathBuf {
    flicker_core::roots::roots()
        .package()
        .join("retarget/clips/locomotion")
}
/// Root the Pack Browser scans for `*.pack.json` — the character tree.
fn content_characters() -> PathBuf {
    flicker_core::roots::roots().package().join("characters")
}

/// The clip-row doll — the design's 34px Stage — and the row that carries it.
const CLIP_STAGE: f32 = 34.0;
const CLIP_ROW_H: f32 = 40.0;
/// Visible rows in the clip rail's refilled bank — the scroll page size (the old
/// pixel-arithmetic `clip_page` died with the walker-owned layout).
const CLIP_ROWS: usize = 8;
/// Which stage sub-scene the bench's dolls use (a key in the shared `stages` block).
const DOLL_SOURCE: &str = "portrait";
/// Node-id prefix marking a clip-row doll — the scene reads the clip name back off it.
const STAGE_PREFIX: &str = "clipdoll_";
/// Node-id prefix marking a pack-card doll, so the scene can map a slot back to its pack.
const PACK_STAGE_PREFIX: &str = "packdoll_";
/// How many stranded render targets to tolerate before pruning the stage cache.
const STALE_SLOT_SLACK: usize = 8;
/// Pack Browser card metrics (the design's grid of thumbed cards, refilled 4 per row).
const PACK_CARD_W: f32 = 150.0;
const PACK_CARD_H: f32 = 150.0;
const PACK_CARD_GAP: f32 = 12.0;
const PACK_GRID_COLS: usize = 4;
/// The card's Stage doll — the design's 92px thumb.
const PACK_STAGE: f32 = 92.0;
/// Characters per wrapped line in the detail pane's note, and how many lines the
/// authored tree declares (`pack_note_0..7`).
const NOTE_WRAP: usize = 34;
const NOTE_MAX_LINES: usize = 8;
/// Node id of the TAE preview doll, so the scene can pose it from the edited clip.
const TAE_STAGE_ID: &str = "taedoll";
/// Pointer grab radius for picking an event bar on the timeline, in pixels.
const TAE_GRAB: f32 = 4.0;
/// One press of the capsule stepper, in metres.
const CAPSULE_STEP_M: f32 = 0.05;
/// One press of the parry-window-scale stepper.
const PARRY_SCALE_STEP: f32 = 0.05;
/// The TAE strips' header line, above the ruler and lanes (drawn inside the
/// walker-reserved rtt rects).
const TAE_HEADER_H: f32 = 20.0;
/// Zoom per wheel notch over the graph.
const ZOOM_STEP: f32 = 0.12;
/// Clip rows the rail moves per wheel notch.
const CLIP_SCROLL_ROWS: usize = 3;
/// How far a self-transition's loop arc rises above its card, at zoom 1.
const SELF_LOOP_LIFT: f32 = 20.0;

/// The editor scene.
pub struct LoomforgeBench {
    doc: Option<EditorDoc>,
    /// Why the document failed to load, shown in the body instead of the page.
    load_error: Option<String>,
    tab: Tab,
    /// Last status line (save result / validation summary).
    status: String,

    ui_state: UiState,
    ui_styles: serde_json::Value,
    /// The AUTHORED tree off the manifest's def (the five-line split), walked every
    /// frame; its named container cells are REFILLED at event time. `take`n around
    /// the walk so the walker can borrow it beside the mutable UI state.
    authored: Option<UiNode>,
    /// The pair script (`loomforge.lua`) — derives the page gates and every wash
    /// from the raw Model each frame. `None` only if it failed to load; the pages
    /// then all gate off and the bench shows chrome only.
    script: Option<ScriptHost>,
    /// The screen's declared signal bindings (S9), read off the authored root ONCE.
    ui_intents: UiIntents,
    /// Intent names fired last frame — republished ONCE into the next frame's
    /// Model as the transient `sig_<name>` mirror (S9), then dropped.
    fired_sigs: Vec<String>,
    hud_commands: Vec<HudCommand>,
    hud_white: Option<TextureHandle>,
    theme: Option<Theme>,

    /// The walker-reserved strip rects this frame (`lf_tae_strip` on the SM page,
    /// `lf_tae_page_strip` on the TAE page) — the scene draws + picks inside them,
    /// so the timeline and the pointer can never disagree about the band.
    sm_strip_rect: Option<StageRect>,
    page_strip_rect: Option<StageRect>,

    /// This frame's state-card rects — the scene-owned canvas geometry, recomputed in
    /// `update` so hit-testing (select / drop) and `render` agree on one layout.
    cards: Vec<CardRect>,
    /// The rect the cards live in.
    canvas_area: CanvasArea,
    /// This frame's transition geometry, built once so picking and drawing agree.
    edges: Vec<Edge>,
    /// The transition being edited, if any. Selecting an edge swaps the pack rail over to
    /// its inspector — you are editing one edge, not binding clips.
    selected_edge: Option<EdgeRef>,

    /// Active canvas tool.
    tool: Tool,
    /// While the Link tool is mid-drag: the source card.
    link_from: Option<usize>,
    /// Trigger stamped on transitions the Link tool creates (cycled from the rail).
    link_trigger: Trigger,
    /// Previous frame's held state, so a release edge can be detected.
    prev_down: bool,
    /// Last cursor position — `render` needs it for the link rubber-band.
    cursor: Vec2,

    /// How the author has the graph panned/zoomed, and any cards they placed by hand.
    /// Editor-side only — never written into the pack.
    view: canvas::View,
    /// Card being dragged, with the grab offset in canvas-local space so it moves under
    /// the cursor rather than snapping its corner there.
    drag_card: Option<(usize, Vec2)>,
    /// Cursor position when a canvas pan began.
    pan_from: Option<Vec2>,
    /// First clip row shown — the rail scrolls a 91-clip library instead of truncating it.
    clip_scroll: usize,

    /// The live-preview dolls: one uploaded rig plus a render target per stage slot.
    stage_rig: StageRig,
    /// Doll play-head, seconds. ONE clock drives every stage — each clip loops on its
    /// own duration, so a shared time needs a single add per frame rather than per doll.
    time: f32,
    /// This frame's stage requests, built in `update` and consumed in `render`.
    stage_reqs: Vec<StageReq>,
    /// The slot the pointer was inside last frame — the hovered clip row is the one that
    /// animates. Resolved from the previous frame's rects, so hover costs no extra pass.
    hot_stage: Option<String>,

    /// The Pack Browser's library — every `*.pack.json` under the content tree, scanned
    /// once on `enter`. Real files only; nothing here is fabricated.
    packs: Vec<packs::PackEntry>,
    /// Index into the FILTERED list, so selection always addresses something visible.
    pack_sel: usize,
    /// Active filter-rail selections. Empty = unfiltered (a freshly-opened browser shows
    /// everything rather than nothing).
    pack_kinds: Vec<packs::PackKind>,
    pack_skels: Vec<String>,

    /// The TAE Editor's selected event. Carries its owning state, so the inspector can
    /// never end up editing an event that belongs to a different state than the timeline
    /// is showing.
    tae_event: Option<doc::EventRef>,
    /// Transport: the TAE page's playhead advances only while playing, so an author can
    /// park on a frame and step it. The dolls keep running off the same clock.
    tae_playing: bool,
}

/// A transition's on-screen geometry, built once per frame so that what is drawn and what
/// the pointer picks can never disagree.
struct Edge {
    id: EdgeRef,
    /// A straight edge is one segment; a self-transition is a three-segment loop arc over
    /// the card, because a straight line to itself would vanish inside the card.
    segs: [(Vec2, Vec2); 3],
    n: usize,
}

impl Edge {
    /// The segment the pointer picks against — the middle one, which is the whole edge
    /// for a straight run and the top bar of a self-loop.
    fn pick(&self) -> (Vec2, Vec2) {
        self.segs[self.n / 2]
    }
}

impl Default for LoomforgeBench {
    fn default() -> Self {
        Self::shipped()
    }
}

/// Find the first descendant (or self) with `id`, mutably — the seam the bench refills
/// its named container cells through (the sablework Rust-fills-the-container pattern;
/// there is no shared helper, so this is the local one).
fn find_by_id_mut<'a>(node: &'a mut UiNode, id: &str) -> Option<&'a mut UiNode> {
    if node.id == id {
        return Some(node);
    }
    node.children.iter_mut().find_map(|c| find_by_id_mut(c, id))
}

impl LoomforgeBench {
    /// The runtime constructor — the manifest hands in the authored `SceneDef`
    /// (the five-line split): the tree + this bench's style blocks come from
    /// `loomforge.scene.json`.
    pub fn new(def: &SceneDef) -> Self {
        Self::from_parts(def.tree.clone(), def.styles.clone())
    }

    /// A bench on the SHIPPED scene file — the seam a test drives without an
    /// app, exercising the same authored tree the runtime gets.
    #[cfg(test)]
    pub fn shipped() -> Self {
        let def = SceneDef::parse("loomforge", LF_SCENE)
            .expect("the shipped loomforge.scene.json parses");
        Self::from_parts(def.tree, def.styles)
    }

    #[cfg(not(test))]
    pub fn shipped() -> Self {
        // Outside tests the manifest is the only construction path; a def-less
        // bench would be a blank screen, so `Default` routes here loudly.
        unreachable!("LoomforgeBench is built from the manifest's SceneDef")
    }

    fn from_parts(authored: Option<UiNode>, scene_styles_json: Option<serde_json::Value>) -> Self {
        if authored.is_none() {
            tracing::error!("loomforge: the scene def declares no `tree` — no UI will draw");
        }
        let ui_styles = flicker::ui::load_shared_styles(scene_styles_json.as_ref());
        // The screen's declared bindings (S9), read off the authored root ONCE.
        let ui_intents = authored.as_ref().map(UiIntents::of).unwrap_or_default();
        let script = match ScriptHost::new(LF_SCRIPT, "loomforge.lua") {
            Ok(h) => Some(h),
            Err(e) => {
                tracing::error!("loomforge.lua failed to load — chrome only: {e}");
                None
            }
        };
        Self {
            doc: None,
            load_error: None,
            tab: Tab::default(),
            status: String::new(),
            ui_state: UiState::new(),
            ui_styles,
            authored,
            script,
            ui_intents,
            fired_sigs: Vec::new(),
            hud_commands: Vec::new(),
            hud_white: None,
            theme: None,
            sm_strip_rect: None,
            page_strip_rect: None,
            cards: Vec::new(),
            canvas_area: CanvasArea {
                pos: Vec2::ZERO,
                size: Vec2::ZERO,
            },
            edges: Vec::new(),
            selected_edge: None,
            tool: Tool::default(),
            link_from: None,
            link_trigger: Trigger::ClipDone,
            prev_down: false,
            cursor: Vec2::ZERO,
            view: canvas::View::default(),
            drag_card: None,
            pan_from: None,
            clip_scroll: 0,
            stage_rig: StageRig::new(),
            time: 0.0,
            stage_reqs: Vec::new(),
            hot_stage: None,
            packs: Vec::new(),
            pack_sel: 0,
            pack_kinds: Vec::new(),
            pack_skels: Vec::new(),
            tae_event: None,
            tae_playing: true,
        }
    }

    /// The filtered library the grid and the detail pane both read, so a card and the
    /// inspector beside it can never disagree about which pack is selected.
    fn visible_packs(&self) -> Vec<&packs::PackEntry> {
        packs::filter(&self.packs, &self.pack_kinds, &self.pack_skels, "")
    }

    fn selected_pack(&self) -> Option<&packs::PackEntry> {
        let v = self.visible_packs();
        v.get(self.pack_sel.min(v.len().saturating_sub(1))).copied()
    }

    fn selected_is(&self, i: usize) -> bool {
        self.doc.as_ref().and_then(|d| d.selected()) == Some(i)
    }

    /// The document, once loaded.
    pub fn doc(&self) -> Option<&EditorDoc> {
        self.doc.as_ref()
    }

    /// Which page is showing.
    pub fn tab(&self) -> Tab {
        self.tab
    }

    // ── the Model (the RAW publish the pair script derives over) ────────────────

    /// The RAW runtime variables the pair script and the tree bind. Display copy is
    /// published as resolved `$token`s (localised here, the Model-channel strings
    /// gate); numbers and wire names ride as data. Presentation logic (page gates,
    /// washes) belongs to `loomforge.lua`'s `derive()`, not here.
    fn hud_model(&self) -> ValueMap {
        let r = |t: &str| strings::resolve(t).into_owned();
        let mut m = ValueMap::default();

        // The page cursor (a NUMBER — 1B64FF03) and the active tool's id (a name).
        let tab_i = Tab::ALL.iter().position(|t| *t == self.tab).unwrap_or(0);
        m.set("sel_tab", tab_i as f64);
        m.set("tool", self.tool.id());

        // Top bar.
        m.set(
            "save_label",
            r(if self.is_dirty() {
                "$lf_save_dirty"
            } else {
                "$lf_save"
            }),
        );
        m.set("rig_badge", self.rig_badge());
        // The status line is GLOBAL now (it used to render only on the Creature
        // fallback page — every save/validate/bind message was invisible).
        m.set(
            "lf_status",
            if !self.status.is_empty() {
                self.status.clone()
            } else if let Some(err) = &self.load_error {
                format!("{} {err}", r("$lf_could_not_load"))
            } else if self.doc.is_none() {
                r("$lf_loading")
            } else {
                String::new()
            },
        );

        // ── SM page: the pack rail's two modes + the clip window. ──
        m.set("has_edge", self.selected_edge.is_some());
        m.set("pack_summary", self.pack_summary());
        m.set(
            "link_trigger_label",
            format!("{} {}", r("$lf_link_on"), trigger_label(self.link_trigger)),
        );
        let total = self.doc.as_ref().map_or(0, |d| d.clip_names().count());
        let first = self.clip_scroll.min(total.saturating_sub(1));
        let last = (first + CLIP_ROWS).min(total);
        if let Some(doc) = &self.doc {
            for (i, name) in doc.clip_names().skip(first).take(CLIP_ROWS).enumerate() {
                m.set(format!("clipname_{i}"), name);
                // The hovered row's doll animates; the rest hold their poster.
                if self
                    .hot_stage
                    .as_deref()
                    .and_then(|h| h.strip_prefix(STAGE_PREFIX))
                    == Some(name)
                {
                    m.set(live_key(name), true);
                }
            }
        }
        m.set(
            "clip_scroll_line",
            if total > CLIP_ROWS {
                format!(
                    "{}\u{2013}{} {} {total} · {}",
                    first + 1,
                    last,
                    r("$lf_of"),
                    r("$lf_scroll_for_more")
                )
            } else {
                String::new()
            },
        );
        m.set("clip_prev_enabled", first > 0);
        m.set("clip_next_enabled", first + CLIP_ROWS < total);

        // The transition inspector's readouts (its subtree gates on `rail_edge`).
        self.edge_model(&mut m, &r);

        // ── Pack Browser page. ──
        let counts = packs::kind_counts(&self.packs);
        for (i, k) in packs::PackKind::ALL.iter().enumerate() {
            m.set(
                format!("packkind_{i}_label"),
                format!("{}  {}", r(kind_token(*k)), counts[i]),
            );
            m.set(format!("packkind_{i}_on"), self.pack_kinds.contains(k));
        }
        let skels = packs::skeletons(&self.packs);
        m.set("skel_count", skels.len() as f64);
        for (i, sk) in skels.iter().enumerate() {
            m.set(format!("skel_{i}_label"), sk.clone());
            m.set(format!("skel_{i}_on"), self.pack_skels.contains(sk));
        }
        let vis = self.visible_packs();
        let filtered = !self.pack_kinds.is_empty() || !self.pack_skels.is_empty();
        m.set("pack_visible", vis.len() as f64);
        m.set("pack_sel", self.pack_sel as f64);
        m.set("packs_empty", vis.is_empty());
        m.set(
            "pack_count_line",
            if filtered {
                format!(
                    "{} {} {} {}",
                    vis.len(),
                    r("$lf_of"),
                    self.packs.len(),
                    r("$lf_packs_lc")
                )
            } else {
                format!(
                    "{} {}",
                    vis.len(),
                    r(if vis.len() == 1 {
                        "$lf_pack_lc"
                    } else {
                        "$lf_packs_lc"
                    })
                )
            },
        );
        for (i, e) in vis.iter().enumerate() {
            m.set(format!("packname_{i}"), e.name.clone());
            m.set(
                format!("packmeta_{i}"),
                format!(
                    "{} · {} {}",
                    r(kind_token(e.kind)),
                    e.states,
                    r("$lf_states_lc")
                ),
            );
            m.set(format!("packmeta_{i}_color"), e.kind.color_path());
        }
        self.pack_detail_model(&mut m, &r);

        // ── Creature page. ──
        m.set(
            "lf_warnings",
            match self.doc.as_ref().map(|d| d.warnings().len()).unwrap_or(0) {
                0 => String::new(),
                n => format!("{n} {}", r("$lf_warnings_word")),
            },
        );

        // ── TAE page. ──
        self.tae_model(&mut m, &r);
        m
    }

    /// The transition inspector's binds — shown while an edge is selected.
    fn edge_model(&self, m: &mut ValueMap, r: &dyn Fn(&str) -> String) {
        let t = self.selected_edge.and_then(|e| {
            self.doc
                .as_ref()
                .and_then(|d| doc::transition(d.def(), e).map(|t| (d, e, t)))
        });
        let Some((doc, e, t)) = t else {
            for k in [
                "edge_head",
                "edge_trigger_label",
                "edge_prio_line",
                "edge_blend_line",
                "edge_window_line",
            ] {
                m.set(k, "");
            }
            m.set("edge_window_shown", false);
            return;
        };
        let from = doc.states().get(e.from).map_or("?", |s| s.name.as_str());
        m.set("edge_head", format!("{from}  \u{2192}  {}", t.to));
        m.set(
            "edge_trigger_label",
            format!("{} {}", r("$lf_on"), trigger_label(t.on)),
        );
        m.set(
            "edge_prio_line",
            format!("{}  {}", r("$lf_priority"), t.priority),
        );
        m.set(
            "edge_blend_line",
            match t.blend_ticks {
                Some(b) => format!("{}  {b} {}", r("$lf_blend"), r("$lf_ticks")),
                None => format!(
                    "{}  {} ({})",
                    r("$lf_blend"),
                    r("$lf_blend_default"),
                    doc.def().default_blend_ticks
                ),
            },
        );
        m.set("edge_window_shown", t.window.is_some());
        m.set(
            "edge_window_line",
            match t.window {
                Some(w) => format!(
                    "{}  {}\u{2013}{} {}",
                    r("$lf_window"),
                    w.start,
                    w.end,
                    r("$lf_ticks")
                ),
                None => String::new(),
            },
        );
    }

    /// The detail pane's binds — the selected pack's manifest, read off the file.
    fn pack_detail_model(&self, m: &mut ValueMap, r: &dyn Fn(&str) -> String) {
        let Some(e) = self.selected_pack() else {
            for k in [
                "pack_name",
                "pack_kind_line",
                "pack_format",
                "pack_version",
                "pack_skeleton",
                "pack_states",
                "pack_transitions",
                "pack_events",
                "pack_combat",
                "pack_load_label",
            ] {
                m.set(k, "");
            }
            m.set("pack_kind_color", "loomforge.rail_text.color");
            m.set("pack_note_shown", false);
            for i in 0..NOTE_MAX_LINES {
                m.set(format!("pack_note_{i}"), "");
            }
            m.set("pack_open", false);
            return;
        };
        let open = self.doc.as_ref().is_some_and(|d| d.path() == e.path);
        m.set("pack_name", e.name.clone());
        m.set("pack_kind_line", r(kind_token(e.kind)));
        m.set("pack_kind_color", e.kind.color_path());
        m.set(
            "pack_format",
            if e.format.is_empty() {
                "\u{2014}".to_string()
            } else {
                e.format.clone()
            },
        );
        m.set("pack_version", e.version.to_string());
        m.set("pack_skeleton", e.skeleton.clone());
        m.set("pack_states", e.states.to_string());
        m.set("pack_transitions", e.transitions.to_string());
        m.set(
            "pack_events",
            format!(
                "{} {} {} {}",
                e.events,
                r("$lf_in_lc"),
                e.event_states,
                r("$lf_states_lc")
            ),
        );
        m.set("pack_combat", e.combat_events.to_string());
        let lines = wrap_text(&e.note, NOTE_WRAP);
        m.set("pack_note_shown", !e.note.is_empty());
        for i in 0..NOTE_MAX_LINES {
            m.set(
                format!("pack_note_{i}"),
                lines.get(i).cloned().unwrap_or_default(),
            );
        }
        m.set("pack_open", open);
        m.set(
            "pack_load_label",
            r(if open { "$lf_loaded" } else { "$lf_load_pack" }),
        );
    }

    /// The TAE page's binds — the edited clip's axis, the nine lane counts, the two
    /// budget gauges, the transport, and the selected event's authored fields.
    fn tae_model(&self, m: &mut ValueMap, r: &dyn Fn(&str) -> String) {
        let axis = self.tae_axis();
        match &axis {
            Some((_, clip, frames, rate)) => {
                m.set("tae_clip_name", clip.clone());
                // strings-gate-exempt: Hz is a unit symbol, not copy.
                m.set(
                    "tae_clip_axis",
                    format!("{frames} {} · {rate} Hz", r("$lf_frames_lc")),
                );
            }
            None => {
                m.set("tae_clip_name", r("$lf_select_state_prompt"));
                m.set("tae_clip_axis", "");
            }
        }
        let counts = self.tae_lane_counts();
        for (i, c) in counts.iter().enumerate() {
            m.set(format!("lane_{i}_count"), *c as f64);
        }
        self.budget_model(m, r);

        let (frames, rate) = axis.map(|(_, _, f, rt)| (f, rt)).unwrap_or((0, 30));
        let head = self.tae_playhead(frames, rate);
        m.set(
            "tae_frame_line",
            format!("{} {head} / {frames}", r("$lf_frame_word")),
        );
        // Transport glyphs carry no alphabetics — data, not copy.
        m.set("tae_play_glyph", if self.tae_playing { "||" } else { ">" });
        // strings-gate-exempt: the seconds readout is a unit-suffixed number.
        m.set(
            "tae_time",
            format!(
                "{:.2}s",
                self.time % ((frames.max(1) as f32) / rate.max(1) as f32)
            ),
        );

        let Some((ev, lane)) = self.selected_event() else {
            m.set("tae_has_event", false);
            for k in [
                "tae_ev_head",
                "tae_ev_label",
                "tae_start_line",
                "tae_end_line",
                "tae_attach_line",
                "tae_cap_line",
                "tae_damage_line",
                "tae_poise_line",
                "tae_hit_label",
                "tae_resp_head",
                "tae_parry_line",
                "tae_sfx_line",
                "tae_vfx_line",
            ] {
                m.set(k, "");
            }
            m.set("tae_ev_head_color", "loomforge.rail_title.color");
            m.set("tae_resp_head_color", "loomforge.rail_title.color");
            for i in 0..Response::ALL.len() {
                m.set(format!("tae_resp_{i}_label"), "");
                m.set(format!("tae_resp_{i}_on"), false);
            }
            return;
        };
        m.set("tae_has_event", true);
        m.set(
            "tae_ev_head",
            format!("{} {}", r(lane_token(lane)), r("$lf_event_word")),
        );
        m.set(
            "tae_ev_head_color",
            format!("loomforge.tae_lane.{}.swatch", lane.id()),
        );
        m.set(
            "tae_ev_label",
            if ev.label.is_empty() {
                "\u{2014}".to_string()
            } else {
                ev.label.clone()
            },
        );
        m.set(
            "tae_start_line",
            format!("{}   {}", r("$lf_start"), ev.tick),
        );
        m.set(
            "tae_end_line",
            match ev.end {
                Some(e) => format!("{}   {e}", r("$lf_end")),
                None => format!("{}   {}", r("$lf_end"), r("$lf_end_point")),
            },
        );
        let c = ev.combat.as_ref();
        m.set(
            "tae_attach_line",
            format!(
                "{}   {}",
                r("$lf_attach_bone"),
                c.and_then(|c| c.attach_bone.as_deref())
                    .unwrap_or("\u{2014}")
            ),
        );
        // strings-gate-exempt: the metre suffix is a unit symbol, not copy.
        m.set(
            "tae_cap_line",
            format!(
                "{}   {:.2}m",
                r("$lf_capsule"),
                c.and_then(|c| c.capsule_radius).unwrap_or(0.35)
            ),
        );
        m.set(
            "tae_damage_line",
            match c.and_then(|c| c.damage) {
                Some([lo, hi]) => {
                    format!("{}   {lo:.0}\u{2013}{hi:.0}", r("$lf_damage_word"))
                }
                None => format!("{}   \u{2014}", r("$lf_damage_word")),
            },
        );
        m.set(
            "tae_poise_line",
            match c.and_then(|c| c.poise_damage) {
                Some(p) => format!("{}   {p:.0}", r("$lf_poise")),
                None => format!("{}   \u{2014}", r("$lf_poise")),
            },
        );
        m.set(
            "tae_hit_label",
            format!(
                "{} {}",
                r("$lf_hit_type"),
                c.and_then(|c| c.hit_type)
                    .map(hit_type_label)
                    .unwrap_or("\u{2014}")
            ),
        );
        let mask = c.map(|c| c.response_mask).unwrap_or_default();
        m.set(
            "tae_resp_head",
            r(if mask.is_perilous() {
                "$lf_response_perilous"
            } else {
                "$lf_response"
            }),
        );
        m.set(
            "tae_resp_head_color",
            if mask.is_perilous() {
                "loomforge.tae_lane.budget_over"
            } else {
                "loomforge.rail_title.color"
            },
        );
        for (i, resp) in Response::ALL.iter().enumerate() {
            // The wire name — the exact value written into the mask (data, not copy).
            m.set(format!("tae_resp_{i}_label"), response_label(*resp));
            m.set(format!("tae_resp_{i}_on"), mask.allows(*resp));
        }
        // strings-gate-exempt: the × multiplier is a unit symbol, not copy.
        m.set(
            "tae_parry_line",
            format!(
                "{}   {:.2}\u{00d7}",
                r("$lf_parry_scale"),
                c.and_then(|c| c.parry_window_scale).unwrap_or(1.0)
            ),
        );
        m.set(
            "tae_sfx_line",
            format!(
                "{}   {}",
                r("$lf_sfx"),
                c.and_then(|c| c.sfx_cue.as_deref()).unwrap_or("\u{2014}")
            ),
        );
        m.set(
            "tae_vfx_line",
            format!(
                "{}   {}",
                r("$lf_vfx"),
                c.and_then(|c| c.vfx_cue.as_deref()).unwrap_or("\u{2014}")
            ),
        );
    }

    /// The gutter's two budget gauges: `SERVER` = the earliest parry catch window
    /// (min — the tightest window is the one the netcode must survive), `PLAYER` =
    /// the widest telegraph against the tier's floor (max — the most generous
    /// warning the state offers). Only shown for windows that actually set one.
    fn budget_model(&self, m: &mut ValueMap, r: &dyn Fn(&str) -> String) {
        m.set("budget_server_shown", false);
        m.set("budget_player_shown", false);
        for k in ["budget_server_line", "budget_player_line"] {
            m.set(k, "");
        }
        for k in ["budget_server_color", "budget_player_color"] {
            m.set(k, "loomforge.rail_text.color");
        }
        let Some((_, _, _, rate)) = self.tae_axis() else {
            return;
        };
        let Some(doc) = &self.doc else { return };
        let Some(st) = doc.selected().and_then(|i| doc.states().get(i)) else {
            return;
        };

        if let Some(tick) = st
            .events
            .iter()
            .filter(|e| e.kind == EventKind::Parry)
            .map(|e| e.tick)
            .min()
        {
            let (ms, verdict) = tae::parry_budget(tick, rate);
            m.set("budget_server_shown", true);
            // strings-gate-exempt: ms is a unit symbol, not copy.
            m.set(
                "budget_server_line",
                format!(
                    "{} @{tick}  =  {ms:.0} ms {}",
                    r("$lf_parry_word"),
                    r("$lf_commit_horizon")
                ),
            );
            m.set(
                "budget_server_color",
                format!("loomforge.tae_lane.{}", verdict.color_key()),
            );
        }

        if let Some(width) = st
            .events
            .iter()
            .filter(|e| e.kind == EventKind::Telegraph)
            .map(|e| e.end.unwrap_or(e.tick).saturating_sub(e.tick))
            .max()
        {
            let tier = self.authoring_tier();
            let (ms, verdict) = tae::telegraph_budget(width, rate, tier);
            let floor = tae::telegraph_floor_ms(tier);
            let tier_note = match tier {
                Some(t) => format!("{} {t}", r("$lf_tier")),
                // The creature/encounter model that carries `tier` does not exist yet, so
                // the entry rung is used — say so rather than implying a tier was read.
                None => r("$lf_no_tier"),
            };
            m.set("budget_player_shown", true);
            // strings-gate-exempt: ms and the f frame suffix are unit symbols.
            m.set(
                "budget_player_line",
                format!(
                    "{} {width}f = {ms:.0} ms  ·  {} {floor:.0} ms ({tier_note})",
                    r("$lf_telegraph_word"),
                    r("$lf_floor_word")
                ),
            );
            m.set(
                "budget_player_color",
                format!("loomforge.tae_lane.{}", verdict.color_key()),
            );
        }
    }

    /// The frame's full model: the raw variables plus the pair script's derived
    /// presentation values (page gates + washes) folded over them, and the
    /// transient `sig_<name>` mirror (S9) riding the same ONE publish.
    fn model(&mut self) -> ValueMap {
        let raw = self.hud_model();
        let mut m = raw.clone();
        if let Some(script) = &self.script {
            if let Err(e) = script.set_model(&raw) {
                tracing::error!("loomforge: publishing the model to the script failed: {e}");
            }
            match script.derive() {
                Ok(Some(derived)) => m.extend(derived),
                Ok(None) => {}
                Err(e) => tracing::error!("loomforge: derive() failed: {e}"),
            }
        }
        UiIntents::mirror_into(&mut m, &self.fired_sigs);
        m
    }

    // ── the container refills (the sablework pattern, at EVENT time) ────────────

    /// Rebuild the clip rail's rows for the current scroll window — on load and on
    /// scroll, never per frame. Content-keyed ids (`clip_<name>` / `clipdoll_<name>`)
    /// keep the doll slots honest poster-cache keys and the rows drag sources.
    fn refill_clip_rows(&mut self) {
        let names: Vec<String> = self
            .doc
            .as_ref()
            .map(|d| {
                d.clip_names()
                    .skip(self.clip_scroll)
                    .take(CLIP_ROWS)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        let Some(cell) = self
            .authored
            .as_mut()
            .and_then(|t| find_by_id_mut(t, "lf_clip_rows"))
        else {
            return;
        };
        cell.children = names
            .iter()
            .enumerate()
            .map(|(i, n)| clip_row(i, n))
            .collect();
    }

    /// Rebuild the Pack Browser's card grid — on scan and on a filter change.
    fn refill_pack_cards(&mut self) {
        let vis: Vec<(String,)> = self
            .visible_packs()
            .iter()
            .map(|e| (e.name.clone(),))
            .collect();
        let Some(cell) = self
            .authored
            .as_mut()
            .and_then(|t| find_by_id_mut(t, "lf_pack_cards"))
        else {
            return;
        };
        cell.children = vis
            .chunks(PACK_GRID_COLS)
            .enumerate()
            .map(|(row_i, chunk)| {
                let mut row = node("row");
                row.id = format!("packrow_{row_i}");
                row.gap = PACK_CARD_GAP;
                row.size = Some(PACK_CARD_H);
                row.children = (0..chunk.len())
                    .map(|k| pack_card(row_i * PACK_GRID_COLS + k))
                    .collect();
                row
            })
            .collect();
    }

    /// Rebuild the skeleton filter rows — on scan (the set only changes then).
    fn refill_skel_rows(&mut self) {
        let n = packs::skeletons(&self.packs).len();
        let Some(cell) = self
            .authored
            .as_mut()
            .and_then(|t| find_by_id_mut(t, "lf_skel_rows"))
        else {
            return;
        };
        cell.children = (0..n)
            .map(|i| {
                let mut b = node("button");
                b.id = format!("packskel_{i}");
                b.action = Some(format!("packskel_{i}"));
                b.size = Some(24.0);
                b.tab_group = "lf_pack_filter".to_string();
                b.nav_ordinal = 10 + i as u32;
                b = prop(b, "size_class", text_val("sm"));
                b = prop(b, "label_bind", text_val(format!("skel_{i}_label")));
                prop(b, "style_bind", text_val(format!("skel_{i}_sty")))
            })
            .collect();
    }

    /// The clip axis the TAE page is editing: the selected state's clip, its frame count,
    /// and its tick rate. Everything on the page — ruler, playhead, transport — reads this
    /// one source, so the timeline and the doll can never disagree about the clip.
    fn tae_axis(&self) -> Option<(usize, String, u32, u32)> {
        let doc = self.doc.as_ref()?;
        let i = doc.selected()?;
        let st = doc.states().get(i)?;
        let (frames, rate) = doc.clip_axis(&st.clip)?;
        Some((i, st.clip.clone(), frames, rate))
    }

    /// Current playhead frame, from the shared clock wrapped over the clip's length.
    fn tae_playhead(&self, frames: u32, rate: u32) -> u32 {
        if frames == 0 {
            return 0;
        }
        let fps = if rate == 0 { 30.0 } else { rate as f32 };
        ((self.time * fps) as u32) % frames
    }

    /// The authoring tier that bounds how tight a telegraph may be.
    ///
    /// `None` today: `tier` belongs on the creature/encounter, and that model lands with the
    /// combat system. Until then the ladder's entry rung applies — the safe end.
    fn authoring_tier(&self) -> Option<u32> {
        None
    }

    /// Events per lane for the selected state — the track list's counts. Sized from
    /// `Lane::ALL` rather than a literal, so adding a lane cannot leave this behind.
    fn tae_lane_counts(&self) -> Vec<usize> {
        let mut counts = vec![0usize; tae::Lane::ALL.len()];
        let Some(doc) = &self.doc else { return counts };
        let Some(st) = doc.selected().and_then(|i| doc.states().get(i)) else {
            return counts;
        };
        for ev in &st.events {
            let lane = tae::lane_of(ev.kind);
            if let Some(i) = tae::Lane::ALL.iter().position(|l| *l == lane) {
                counts[i] += 1;
            }
        }
        counts
    }

    /// The selected event and the lane it lives in, validated against the CURRENT document
    /// so a stale selection (after a delete, or a state swap) resolves to nothing rather
    /// than to whatever now sits at that index.
    fn selected_event(&self) -> Option<(&flicker_skeletal::state::EventDef, tae::Lane)> {
        let doc = self.doc.as_ref()?;
        let e = self.tae_event?;
        if doc.selected() != Some(e.state) {
            return None;
        }
        let ev = doc::event(doc.def(), e)?;
        Some((ev, tae::lane_of(ev.kind)))
    }

    /// The transition under `p`, if the pointer is close enough to grab one.
    fn hit_edge(&self, p: Vec2) -> Option<EdgeRef> {
        let segs: Vec<(EdgeRef, Vec2, Vec2)> = self
            .edges
            .iter()
            .map(|e| {
                let (a, b) = e.pick();
                (e.id, a, b)
            })
            .collect();
        canvas::hit_edge(&segs, p)
    }

    /// Apply the transition inspector's actions. Each reports through the status line, so
    /// an edit that the document refuses (a stale edge, a clamped dial) says so instead of
    /// looking like it worked.
    fn apply_edge_actions(&mut self, results: &ValueMap) {
        let Some(e) = self.selected_edge else { return };

        if results.is_on("edge_delete") {
            let ok = self.doc.as_mut().is_some_and(|d| d.remove_transition(e));
            self.status = strings::resolve(if ok {
                "$lf_transition_removed"
            } else {
                "$lf_transition_gone"
            })
            .into_owned();
            self.selected_edge = None;
            return;
        }

        let step = |up: &str, down: &str| -> i32 {
            i32::from(results.is_on(up)) - i32::from(results.is_on(down))
        };
        let prio = step("prio_inc", "prio_dec");
        let blend = step("blend_inc", "blend_dec");
        let cycle = results.is_on("edge_trigger");
        if prio == 0 && blend == 0 && !cycle {
            return;
        }
        let Some(doc) = self.doc.as_mut() else { return };
        if prio != 0 {
            doc.nudge_priority(e, prio);
        }
        if blend != 0 {
            doc.nudge_blend(e, blend);
        }
        if cycle {
            doc.cycle_trigger(e);
        }
        self.status = match doc::transition(doc.def(), e) {
            Some(t) => format!(
                "{} · {} {} · {} {}",
                trigger_label(t.on),
                strings::resolve("$lf_priority"),
                t.priority,
                strings::resolve("$lf_blend"),
                match t.blend_ticks {
                    Some(b) => b.to_string(),
                    None => strings::resolve("$lf_blend_default").into_owned(),
                }
            ),
            None => strings::resolve("$lf_transition_gone").into_owned(),
        };
    }

    /// This frame's transition geometry. One pass, consumed by both picking and drawing.
    fn build_edges(&self) -> Vec<Edge> {
        let Some(doc) = &self.doc else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for (i, st) in doc.states().iter().enumerate() {
            let Some(&from) = self.cards.get(i) else {
                continue;
            };
            for (index, t) in st.transitions.iter().enumerate() {
                let Some(j) = doc.state_index(&t.to) else {
                    continue;
                };
                let Some(&to) = self.cards.get(j) else {
                    continue;
                };
                let id = EdgeRef { from: i, index };
                if i == j {
                    // Self-transition: a loop arc over the card's top edge.
                    let lift = SELF_LOOP_LIFT * self.view.zoom;
                    let (x0, x1) = (
                        from.pos.x + from.size.x * 0.35,
                        from.pos.x + from.size.x * 0.65,
                    );
                    let (y0, y1) = (from.pos.y, from.pos.y - lift);
                    out.push(Edge {
                        id,
                        segs: [
                            (Vec2::new(x1, y0), Vec2::new(x1, y1)),
                            (Vec2::new(x1, y1), Vec2::new(x0, y1)),
                            (Vec2::new(x0, y1), Vec2::new(x0, y0)),
                        ],
                        n: 3,
                    });
                } else {
                    let (p, q) = canvas::edge_points(from, to);
                    let zero = (Vec2::ZERO, Vec2::ZERO);
                    out.push(Edge {
                        id,
                        segs: [(p, q), zero, zero],
                        n: 1,
                    });
                }
            }
        }
        out
    }

    /// Move the clip-rail window. Positive delta = wheel up = back toward the first clip.
    /// Clamped so the last page is reachable and the list can never scroll past its end.
    /// A moved window REFILLS the rail's rows (an event, not a per-frame rebuild).
    fn scroll_clips(&mut self, delta: f32) {
        let total = self.doc.as_ref().map_or(0, |d| d.clip_names().count());
        let step = (delta.abs().ceil() as usize).max(1) * CLIP_SCROLL_ROWS;
        let before = self.clip_scroll;
        self.clip_scroll = if delta > 0.0 {
            self.clip_scroll.saturating_sub(step)
        } else {
            (self.clip_scroll + step).min(total.saturating_sub(CLIP_ROWS))
        };
        if self.clip_scroll != before {
            self.refill_clip_rows();
        }
    }

    /// One-line description of the loaded pack for the rail header.
    fn pack_summary(&self) -> String {
        match &self.doc {
            Some(d) => format!(
                "{} · {} {}",
                short_path(d.path()),
                d.clip_names().count(),
                strings::resolve("$lf_clips_lc")
            ),
            None => strings::resolve("$lf_no_pack_loaded").into_owned(),
        }
    }

    /// What the TAE strip reports for the current selection.
    fn tae_summary(&self) -> String {
        let Some(doc) = &self.doc else {
            return strings::resolve("$lf_no_pack").into_owned();
        };
        match doc.selected().and_then(|i| doc.states().get(i)) {
            Some(s) => {
                format!(
                    "{} · {} \"{}\" · {} {}",
                    s.name,
                    strings::resolve("$lf_clip_lc"),
                    s.clip,
                    s.events.len(),
                    strings::resolve("$lf_events_lc")
                )
            }
            None => strings::resolve("$lf_select_state_events").into_owned(),
        }
    }

    fn rig_badge(&self) -> String {
        match &self.doc {
            Some(d) => format!(
                "{} · {} {}",
                short_path(d.path()),
                d.states().len(),
                strings::resolve("$lf_states_lc")
            ),
            None => strings::resolve("$lf_no_pack").into_owned(),
        }
    }

    fn is_dirty(&self) -> bool {
        self.doc.as_ref().is_some_and(|d| d.dirty())
    }

    /// This frame's dolls: the walker-reserved clip-row slots, plus one per state card on
    /// the graph page. Cards are scene-drawn, so their rects come from the canvas layout
    /// rather than from a `stage` node — but they make the identical request either way.
    ///
    /// **A slot's id is its cache key, and encodes everything its image depends on** — so
    /// re-binding a card's clip yields a NEW key and a freshly rendered doll, and a poster
    /// can never go stale under its own state.
    /// The pose a pack card's doll shows: the pack's own initial state's clip when that
    /// pack is the one currently open (so its clips are actually loaded), otherwise the
    /// rest pose. A card never claims to play a clip the bench has not loaded.
    fn pack_card_clip(&self, idx: usize, doc: &EditorDoc) -> Option<usize> {
        let entry = self.visible_packs().get(idx).copied()?;
        if doc.path() != entry.path {
            return None;
        }
        let initial = doc.def().initial.clone();
        let state = doc.states().iter().find(|s| s.name == initial)?;
        doc.clip_index(&state.clip)
    }

    fn build_stage_reqs(&self, slots: Vec<flicker::ui::SurfaceSlot>) -> Vec<StageReq> {
        let Some(doc) = &self.doc else {
            return Vec::new();
        };
        if !self.stage_rig.has_source(DOLL_SOURCE) {
            return Vec::new();
        }

        // Clip rows — the walker already laid these out and resolved their `live_bind`.
        let mut reqs: Vec<StageReq> = slots
            .into_iter()
            .filter_map(|s| {
                // A clip row's id carries the clip NAME; a pack card's carries its index.
                let (clip, live) = if let Some(name) = s.id.strip_prefix(STAGE_PREFIX) {
                    (doc.clip_index(name), s.live)
                } else if let Some(i) = s.id.strip_prefix(PACK_STAGE_PREFIX) {
                    let i: usize = i.parse().ok()?;
                    // Only the selected card animates — a live target is a GPU submit.
                    (self.pack_card_clip(i, doc), i == self.pack_sel)
                } else if s.id == TAE_STAGE_ID {
                    // The TAE preview poses from the clip being edited, and re-renders
                    // only while the transport is running — a parked playhead is a poster.
                    let clip = self
                        .tae_axis()
                        .and_then(|(_, name, _, _)| doc.clip_index(&name));
                    (clip, self.tae_playing)
                } else {
                    return None;
                };
                Some(StageReq {
                    rect: StageRect {
                        pos: Vec2::new(s.x, s.y),
                        size: Vec2::new(s.w, s.h),
                    },
                    // Walker chrome sits one layer above the scene-drawn canvas.
                    layer: s.layer + 1.0,
                    tint: s.tint,
                    live,
                    active: live,
                    clip,
                    time: self.time,
                    source: s.source,
                    id: s.id,
                })
            })
            .collect();

        // State cards. Only the SELECTED card animates: a live target is a GPU submit,
        // and a graph page carries one card per state.
        if self.tab == Tab::StateMachine {
            for (i, st) in doc.states().iter().enumerate() {
                let Some(&c) = self.cards.get(i) else {
                    continue;
                };
                let sel = self.selected_is(i);
                let doll = canvas::card_stage_rect(c, self.view.zoom);
                reqs.push(StageReq {
                    id: format!("card_{}#{}", st.name, st.clip),
                    source: DOLL_SOURCE.to_string(),
                    rect: StageRect {
                        pos: doll.pos,
                        size: doll.size,
                    },
                    layer: 0.0,
                    tint: [1.0; 4],
                    live: sel,
                    clip: doc.clip_index(&st.clip),
                    time: self.time,
                    active: sel,
                });
            }
        }
        reqs
    }

    /// Apply this frame's fired UI actions.
    /// TAE Editor actions: transport, and the selected event's authored fields. Every
    /// edit goes through `EditorDoc`, so it dirties the document and Save writes it back.
    fn apply_tae_actions(&mut self, results: &ValueMap) {
        if results.is_on("tae_play") {
            self.tae_playing = !self.tae_playing;
        }
        // Stepping the transport nudges the shared clock by exactly one frame of the
        // edited clip, so the doll and the playhead move together.
        let rate = self.tae_axis().map(|(_, _, _, r)| r).unwrap_or(30);
        let step = 1.0 / if rate == 0 { 30.0 } else { rate as f32 };
        if results.is_on("tae_next") {
            self.time += step;
        }
        if results.is_on("tae_prev") {
            self.time = (self.time - step).max(0.0);
        }

        let Some(e) = self.tae_event else { return };
        let Some(doc) = self.doc.as_mut() else { return };
        for (action, delta) in [("tae_start_dec", -1), ("tae_start_inc", 1)] {
            if results.is_on(action) {
                doc.nudge_event_tick(e, delta);
            }
        }
        for (action, delta) in [("tae_end_dec", -1), ("tae_end_inc", 1)] {
            if results.is_on(action) {
                doc.nudge_event_end(e, delta);
            }
        }
        for (action, delta) in [
            ("tae_cap_dec", -CAPSULE_STEP_M),
            ("tae_cap_inc", CAPSULE_STEP_M),
        ] {
            if results.is_on(action) {
                doc.nudge_event_capsule(e, delta);
            }
        }
        if results.is_on("tae_hit") {
            doc.cycle_event_hit_type(e);
        }
        for (i, r) in Response::ALL.iter().enumerate() {
            if results.is_on(&format!("tae_resp_{i}")) {
                doc.toggle_event_response(e, *r);
            }
        }
        for (action, delta) in [
            ("tae_pscale_dec", -PARRY_SCALE_STEP),
            ("tae_pscale_inc", PARRY_SCALE_STEP),
        ] {
            if results.is_on(action) {
                doc.nudge_event_parry_scale(e, delta);
            }
        }
    }

    /// Pack Browser actions: card selection, the two filter toggles, and Load.
    ///
    /// Filters are toggles over a set rather than a single choice, and toggling one always
    /// re-clamps the selection, so the detail pane can never end up describing a pack the
    /// grid is no longer showing.
    fn apply_pack_actions(&mut self, results: &ValueMap) {
        let visible = self.visible_packs().len();
        for i in 0..visible {
            if results.is_on(&format!("packcard_{i}")) {
                self.pack_sel = i;
            }
        }
        for (i, k) in packs::PackKind::ALL.iter().enumerate() {
            if results.is_on(&format!("packkind_{i}")) {
                toggle(&mut self.pack_kinds, *k);
                self.pack_sel = 0;
            }
        }
        for (i, s) in packs::skeletons(&self.packs).into_iter().enumerate() {
            if results.is_on(&format!("packskel_{i}")) {
                toggle(&mut self.pack_skels, s);
                self.pack_sel = 0;
            }
        }
        // Clamp after any filter change so `selected_pack` always addresses a visible card.
        let now = self.visible_packs().len();
        if self.pack_sel >= now {
            self.pack_sel = now.saturating_sub(1);
        }
        // A filter change is an EVENT: the card grid refills for the new list.
        let filtered = (0..packs::PackKind::ALL.len())
            .any(|i| results.is_on(&format!("packkind_{i}")))
            || (0..packs::skeletons(&self.packs).len())
                .any(|i| results.is_on(&format!("packskel_{i}")));
        if filtered {
            self.refill_pack_cards();
        }

        if results.is_on("pack_load") {
            self.load_selected_pack();
        }
    }

    /// Swap the bench's document over to the selected pack. The clip library is resolved
    /// from the pack's OWN directory plus the shared base rig, so loading a pack authored
    /// against another character still finds its clips.
    fn load_selected_pack(&mut self) {
        let Some(entry) = self.selected_pack().cloned() else {
            return;
        };
        if self.doc.as_ref().is_some_and(|d| d.path() == entry.path) {
            return; // already open — Load is a no-op, not a reload that would drop edits
        }
        let dir = entry.path.parent().map(Path::to_path_buf);
        let base = base_dir();
        let mut roots: Vec<&Path> = vec![&base];
        if let Some(d) = dir.as_deref() {
            roots.push(d);
        }
        match EditorDoc::load(&entry.path, &roots) {
            Ok(doc) => {
                self.status = format!(
                    "{} {} · {} {}",
                    strings::resolve("$lf_loaded"),
                    entry.name,
                    doc.states().len(),
                    strings::resolve("$lf_states_lc")
                );
                self.doc = Some(doc);
                // The graph's layout and selection belong to the pack that is open.
                self.view = canvas::View::default();
                self.selected_edge = None;
                self.link_from = None;
                self.clip_scroll = 0;
                // A new document is an EVENT: the clip rail's rows refill for it.
                self.refill_clip_rows();
            }
            Err(e) => {
                self.status = format!(
                    "{} {}: {e}",
                    strings::resolve("$lf_could_not_load"),
                    entry.name
                );
            }
        }
    }

    fn apply_actions(&mut self, results: &ValueMap) {
        for t in Tab::ALL {
            if results.is_on(t.id()) {
                self.tab = t;
            }
        }
        // The declared `on_tab_next` / `on_tab_prev` intents CYCLE the pages (with
        // wrap), so the bumpers walk the tab bar without a button per page.
        let cycled = i32::from(results.is_on("tab_next")) - i32::from(results.is_on("tab_prev"));
        if cycled != 0 {
            let n = Tab::ALL.len() as i32;
            let i = Tab::ALL.iter().position(|t| *t == self.tab).unwrap_or(0) as i32;
            self.tab = Tab::ALL[((i + cycled).rem_euclid(n)) as usize];
        }
        for t in Tool::ALL {
            if results.is_on(t.id()) {
                self.tool = t;
                self.link_from = None; // switching tools abandons a half-drawn edge
            }
        }
        // The declared `on_mode_next` / `on_mode_prev` intents CYCLE the canvas tools.
        let tooled = i32::from(results.is_on("tool_next")) - i32::from(results.is_on("tool_prev"));
        if tooled != 0 {
            let n = Tool::ALL.len() as i32;
            let i = Tool::ALL.iter().position(|t| *t == self.tool).unwrap_or(0) as i32;
            self.tool = Tool::ALL[((i + tooled).rem_euclid(n)) as usize];
            self.link_from = None;
        }
        // The clip rail's pager buttons ride the same window the wheel scrolls.
        if results.is_on("clip_prev") {
            self.scroll_clips(1.0);
        }
        if results.is_on("clip_next") {
            self.scroll_clips(-1.0);
        }
        if results.is_on("cycle_trigger") {
            self.link_trigger = next_trigger(self.link_trigger);
        }
        self.apply_pack_actions(results);
        self.apply_tae_actions(results);
        self.apply_edge_actions(results);
        if results.is_on("save") {
            // Saving is the one action that must see every pending edit, so it runs after
            // the inspector's have been applied above.
            self.status = match self.doc.as_mut() {
                Some(doc) => match doc.save() {
                    Ok(()) => {
                        format!(
                            "{} {}",
                            strings::resolve("$lf_saved"),
                            short_path(doc.path())
                        )
                    }
                    Err(e) => format!("{} {e}", strings::resolve("$lf_save_failed")),
                },
                None => strings::resolve("$lf_nothing_to_save").into_owned(),
            };
        }
        if results.is_on("validate") {
            self.status = match &self.doc {
                Some(doc) if doc.warnings().is_empty() => {
                    strings::resolve("$lf_validate_ok").into_owned()
                }
                Some(doc) => format!(
                    "{} {}",
                    strings::resolve("$lf_validate_prefix"),
                    doc.warnings().join(" · ")
                ),
                None => strings::resolve("$lf_nothing_to_validate").into_owned(),
            };
        }
    }
}

impl Scene for LoomforgeBench {
    fn enter(&mut self, renderer: &mut Renderer) {
        self.theme = Some(Theme::build(renderer));
        self.hud_white = Some(renderer.load_texture(&[0xff, 0xff, 0xff, 0xff], 1, 1));
        // `ui_styles` was merged in `from_parts` — the scene file's own blocks over
        // the shared theme (the five-line split).

        // The Pack Browser's library: every real `*.pack.json` under the content tree.
        // Scanned once — the browser reads files, it does not watch them. The scan is
        // an EVENT, so the filter rows + card grid refill here.
        self.packs = packs::scan_packs(&content_characters());
        tracing::info!(packs = self.packs.len(), "loomforge: pack library scanned");
        self.refill_skel_rows();
        self.refill_pack_cards();

        match EditorDoc::load(&pack_path(), &[&base_dir(), &clips_dir()]) {
            Ok(doc) => {
                tracing::info!(
                    states = doc.states().len(),
                    clips = doc.clip_names().count(),
                    "loomforge: pack loaded"
                );
                // The dolls share ONE uploaded rig — the GPU skins each instance from its
                // own bone palette, so a screen full of stages costs one mesh, not N.
                self.stage_rig.load(renderer, doc.model(), &self.ui_styles);
                self.doc = Some(doc);
            }
            Err(e) => {
                tracing::warn!("loomforge: pack load failed: {e:?}");
                self.load_error = Some(format!("{e}"));
            }
        }
        self.refill_clip_rows();
    }

    fn update(
        &mut self,
        dt: Duration,
        input: &InputState,
        signals: &mut SceneInput,
        renderer: &Renderer,
    ) -> Transition {
        let screen = renderer.size();
        self.cursor = input.mouse_position;
        // One clock for every doll — each clip loops on its own duration.
        self.time += dt.as_secs_f32();

        let Some(tree) = self.authored.take() else {
            return Transition::None;
        };
        // The scene is DATA: walk the AUTHORED tree (its container cells refilled at
        // event time) with the raw model + the pair script's derived gates/washes.
        let model = self.model();
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
        let frame = run_ui(&tree, &model, &self.ui_styles, &snap, &mut self.ui_state);
        // Copy out what the canvas needs before `frame` is consumed / `self` mutated.
        let over_hud = frame.results.is_on("hud_hit");
        let dropped = frame.results.is_on("drag_dropped");
        let drag_is_clip = frame.results.text("drag_kind") == Some("clip");
        let drag_id = frame.results.text("drag_id").map(str::to_string);
        // The walker-reserved rects: the graph canvas and the two timeline strips —
        // the scene draws + picks inside exactly these, so its geometry and the
        // walker's layout can never disagree (the old triple screen-size recompute
        // died with them). The side rail's resolved box routes the wheel below.
        let canvas_rect = frame.surface_rect("lf_canvas");
        self.sm_strip_rect = frame.surface_rect("lf_tae_strip");
        self.page_strip_rect = frame.surface_rect("lf_tae_page_strip");
        let rail_rect = frame
            .rects
            .iter()
            .find(|(id, _)| id == "lf_rail")
            .map(|(_, r)| *r);
        let mut results = frame.results.clone();
        self.hud_commands = frame.commands;

        // Canvas geometry off THIS frame's reserved rect, so hit-testing (select /
        // drop) and the draw pass agree on one layout.
        self.canvas_area = canvas_rect
            .map(|r| CanvasArea {
                pos: r.pos,
                size: r.size,
            })
            .unwrap_or(CanvasArea {
                pos: Vec2::ZERO,
                size: Vec2::ZERO,
            });
        self.cards = match &self.doc {
            Some(d) => canvas::layout(
                d.states().iter().map(|s| s.name.as_str()),
                self.canvas_area,
                &self.view,
            ),
            None => Vec::new(),
        };
        self.edges = self.build_edges();

        // ── The input seam (input-P3): the PUMP resolved this frame's events — the
        // scene owns no Resolver. One dispatch through the walker, which owns the
        // focus graph, consumes the pointer while it is over the HUD, and fires the
        // screen's DECLARED intents (`on_menu` / `on_tab_*` / `on_mode_*`) as result
        // names. The bespoke canvas/timeline tools below stay scene-owned
        // raw-pointer logic, gated on `over_hud` + the reserved rects. ──
        let mut walker = WalkerHandler::hud(&mut self.ui_state, over_hud)
            .with_nav(&tree, &model)
            .with_rects(&frame.rects)
            .with_intents(&self.ui_intents);
        {
            let mut chain: [&mut dyn InputHandler; 1] = [&mut walker];
            Router::dispatch(signals.events, &mut chain, signals.route);
        }
        // The screen's fired intents (S9), drained once: folded into the results so
        // both input channels reach the ONE dispatch identically, and queued for the
        // one-frame `sig_<name>` Model mirror.
        self.fired_sigs = walker.take_fired();
        let slots = frame.surfaces;
        self.authored = Some(tree);
        for name in &self.fired_sigs {
            results.set(name.clone(), true);
        }

        self.apply_actions(&results);

        // Dolls for this frame, and which one the pointer is inside (for the next).
        self.stage_reqs = self.build_stage_reqs(slots);
        self.hot_stage = self
            .stage_reqs
            .iter()
            .find(|r| r.id.starts_with(STAGE_PREFIX) && rect_contains(&r.rect, self.cursor))
            .map(|r| r.id.clone());

        // The screen DECLARED `on_menu = "pause_open"` (S9): the walker layer consumed
        // the Menu press and fired the name; the ONE dispatch maps it onto the shell
        // pause overlay. The pause overlay shows the PROFILE's map — the pump owns
        // bindings now (input-P3), the scene holds none.
        if results.is_on("pause_open") {
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

        let r = |t: &str| strings::resolve(t).into_owned();
        if self.tab == Tab::StateMachine {
            let hit = canvas::hit_test(&self.cards, input.mouse_position);
            let released = self.prev_down && !input.mouse_left;

            // ── Navigating the workspace ──────────────────────────────────────────
            // The wheel means "more of what is under the cursor": over the clip library
            // it pages the list, over the graph it zooms about the pointer.
            if input.mouse_wheel_delta != 0.0 {
                let over_rail = rail_rect.is_some_and(|[x, y, w, h]| {
                    self.cursor.x >= x
                        && self.cursor.x <= x + w
                        && self.cursor.y >= y
                        && self.cursor.y <= y + h
                });
                if over_rail {
                    self.scroll_clips(input.mouse_wheel_delta);
                } else if self.canvas_area.contains(self.cursor) {
                    let factor = 1.0 + input.mouse_wheel_delta * ZOOM_STEP;
                    self.view.zoom_at(self.canvas_area, self.cursor, factor);
                }
            }
            // Middle-drag pans, whichever tool is active — so panning never competes with
            // what the left button means on this tool.
            if input.mouse_middle {
                if let Some(from) = self.pan_from {
                    self.view.pan += self.cursor - from;
                }
                self.pan_from = Some(self.cursor);
            } else {
                self.pan_from = None;
            }

            // Move a grabbed card. The position is stored by state NAME, editor-side —
            // the pack has no notion of where a state sits on screen.
            if let Some((i, grab)) = self.drag_card {
                if input.mouse_left {
                    let local = self.view.to_local(self.canvas_area, self.cursor) - grab;
                    if let Some(name) = self.doc.as_ref().and_then(|d| d.states().get(i)) {
                        self.view.placed.insert(name.name.clone(), local);
                    }
                } else {
                    self.drag_card = None;
                }
            }

            // Canvas tools. Gated on `!over_hud` so a rail click never also acts on the
            // card behind it, AND on the reserved canvas rect — chrome coverage alone
            // would let a click elsewhere place or delete a state.
            let in_canvas = self.canvas_area.contains(input.mouse_position);
            if input.mouse_left_pressed && !over_hud && in_canvas {
                match self.tool {
                    Tool::Select => match hit {
                        Some(i) => {
                            if let Some(doc) = self.doc.as_mut() {
                                doc.select(Some(i));
                            }
                            self.selected_edge = None;
                            // Grab it where it was clicked, so it tracks the pointer
                            // instead of snapping its corner under the cursor.
                            let grab = self.view.to_local(self.canvas_area, self.cursor)
                                - self.view.to_local(self.canvas_area, self.cards[i].pos);
                            self.drag_card = Some((i, grab));
                        }
                        // Cards are on top, so an edge is only picked where no card is.
                        None => {
                            self.selected_edge = self.hit_edge(self.cursor);
                            if let Some(e) = self.selected_edge {
                                self.status = match self
                                    .doc
                                    .as_ref()
                                    .and_then(|d| doc::transition(d.def(), e))
                                {
                                    Some(t) => format!(
                                        "{} {} {}",
                                        r("$lf_editing_edge"),
                                        trigger_label(t.on),
                                        r("$lf_edge_lc")
                                    ),
                                    None => String::new(),
                                };
                            }
                        }
                    },
                    // Press starts an edge; the release below completes it.
                    Tool::Link => self.link_from = hit,
                    Tool::AddState => {
                        if hit.is_none() {
                            let clip = self
                                .doc
                                .as_ref()
                                .and_then(|d| d.clip_names().next())
                                .map(str::to_string);
                            self.status = match clip {
                                Some(c) => match self.doc.as_mut().and_then(|d| d.add_state(&c)) {
                                    Some(i) => format!(
                                        "{} {} ({} \"{c}\")",
                                        r("$lf_added_state"),
                                        i + 1,
                                        r("$lf_clip_lc")
                                    ),
                                    None => r("$lf_could_not_add_state"),
                                },
                                None => r("$lf_no_clips_for_state"),
                            };
                        }
                    }
                    // Delete acts on whatever is under the pointer: a card removes the
                    // state (and every reference to it), an edge removes just that one
                    // transition and leaves both states standing.
                    Tool::Delete => match hit {
                        Some(i) => {
                            let name = self
                                .doc
                                .as_ref()
                                .and_then(|d| d.states().get(i))
                                .map(|s| s.name.clone())
                                .unwrap_or_default();
                            if self.doc.as_mut().is_some_and(|d| d.remove_state(i)) {
                                self.status = format!(
                                    "{} {name} {}",
                                    r("$lf_removed"),
                                    r("$lf_and_its_edges")
                                );
                                self.selected_edge = None;
                            }
                        }
                        None => {
                            if let Some(e) = self.hit_edge(self.cursor) {
                                if self.doc.as_mut().is_some_and(|d| d.remove_transition(e)) {
                                    self.status = r("$lf_transition_removed");
                                    self.selected_edge = None;
                                }
                            }
                        }
                    },
                }
            }

            // Link release: land on a DIFFERENT card to weave the transition.
            if released && self.tool == Tool::Link {
                if let (Some(from), Some(to)) = (self.link_from, hit) {
                    let on = self.link_trigger;
                    self.status = if from == to {
                        r("$lf_no_self_link")
                    } else if self
                        .doc
                        .as_mut()
                        .is_some_and(|d| d.add_transition(from, to, on))
                    {
                        format!("{} {}", r("$lf_linked_on"), trigger_label(on))
                    } else {
                        format!(
                            "{} {} {}",
                            r("$lf_edge_exists_prefix"),
                            trigger_label(on),
                            r("$lf_edge_exists_suffix")
                        )
                    };
                }
                self.link_from = None;
            }
            // A clip dropped onto a card binds it — the drag channel closing the loop
            // into the document (and marking it dirty for Save).
            if dropped && drag_is_clip {
                if let Some(id) = drag_id {
                    self.status = match canvas::hit_test(&self.cards, input.mouse_position) {
                        Some(i) => {
                            let name = self
                                .doc
                                .as_ref()
                                .and_then(|d| d.states().get(i))
                                .map(|s| s.name.clone())
                                .unwrap_or_default();
                            if self.doc.as_mut().is_some_and(|d| d.bind_clip(i, &id)) {
                                format!("{} \"{id}\" {} {name}", r("$lf_bound"), r("$lf_to_lc"))
                            } else {
                                format!(
                                    "{} \"{id}\" {}",
                                    r("$lf_could_not_bind"),
                                    r("$lf_unknown_clip_or_state")
                                )
                            }
                        }
                        None => format!("\"{id}\" {}", r("$lf_dropped_outside")),
                    };
                }
            }
        }

        // TAE Editor: a press on the scene-drawn timeline (never over the chrome) selects
        // the event under the cursor, opening it in the inspector. A press on empty track
        // clears the selection, matching the graph canvas's "click nothing = deselect".
        if self.tab == Tab::TaeEditor && input.mouse_left_pressed && !over_hud {
            if let Some(rect) = self.page_strip_rect {
                if rect_contains(&rect, input.mouse_position) {
                    self.tae_event = self.tae_pick_event(input.mouse_position);
                }
            }
        }
        self.prev_down = input.mouse_left;

        Transition::None
    }

    fn render(&mut self, renderer: &mut Renderer) {
        let base = renderer.layer();

        // Offscreen doll passes FIRST — `FrameGraph::execute` must lead `render()`,
        // because an offscreen pass resets the shared draw queues and would otherwise
        // discard main-frame geometry queued before it.
        if !self.stage_reqs.is_empty() {
            if let Some(doc) = &self.doc {
                let mut fg = FrameGraph::new();
                self.stage_rig
                    .stage(renderer, &mut fg, doc.model(), base, &self.stage_reqs);
                fg.execute(renderer);
            }
        }
        // Amortised cleanup: a page change strands the previous page's targets, but a
        // steady page never trips this, so the common frame does no bookkeeping at all.
        if self.stage_rig.slot_count() > self.stage_reqs.len() + STALE_SLOT_SLACK {
            let shown: Vec<&str> = self.stage_reqs.iter().map(|r| r.id.as_str()).collect();
            self.stage_rig
                .retain_slots(renderer, &|id| shown.contains(&id));
        }

        // The scene-owned canvas paints on the base layer, UNDER the walker chrome —
        // otherwise the rails' panels and the cards would interleave by submission
        // order within one layer (the paperdoll HUD-behind-panels trap). The dolls
        // composited above ride the sprite pass, which runs after ui-panels within a
        // layer, so each lands over its own backdrop without a layer of its own.
        if self.tab == Tab::StateMachine {
            self.draw_canvas(renderer);
            if let Some(rect) = self.sm_strip_rect {
                self.draw_tae(renderer, rect);
            }
        }
        if self.tab == Tab::TaeEditor {
            if let Some(rect) = self.page_strip_rect {
                self.draw_tae_page(renderer, rect);
            }
        }
        if let Some(white) = self.hud_white {
            renderer.set_layer(base + 1.0);
            render_hud(renderer, &self.hud_commands, white, &[]);
            renderer.set_layer(base);
        }
    }
}

impl LoomforgeBench {
    /// Draw the node-graph canvas: ground, transition edges, then the state cards.
    fn draw_canvas(&self, r: &mut Renderer) {
        let Some(doc) = &self.doc else { return };
        let a = self.canvas_area;
        let bg = self.color("loomforge.canvas.bg", [0.03, 0.035, 0.047, 1.0]);
        r.draw_ui_panel(a.pos, a.size, bg, bg, 0.0, 0.0, 0.0, [0.0; 4], 0.0);

        let edge_c = self.color("loomforge.canvas.edge", [0.72, 0.59, 0.35, 1.0]);
        let edge_sel = self.color("loomforge.canvas.edge_sel", [0.435, 0.592, 1.0, 1.0]);
        // Edges first so the cards sit on top of them. The geometry was built in `update`
        // — the same list the pointer picks against, so what lights up is what was hit.
        for e in &self.edges {
            let picked = self.selected_edge == Some(e.id);
            let lit = picked
                || self.selected_is(e.id.from)
                || doc
                    .states()
                    .get(e.id.from)
                    .and_then(|s| s.transitions.get(e.id.index))
                    .and_then(|t| doc.state_index(&t.to))
                    .is_some_and(|j| self.selected_is(j));
            let c = if lit { edge_sel } else { edge_c };
            let w = if picked {
                3.0
            } else if lit {
                2.4
            } else {
                1.4
            };
            for (a, b) in &e.segs[..e.n] {
                draw_line(r, *a, *b, w, c);
            }
        }

        // In-degree for the card's `IN n · OUT n` meta line.
        let mut in_deg = vec![0usize; doc.states().len()];
        for st in doc.states() {
            for t in &st.transitions {
                if let Some(j) = doc.state_index(&t.to) {
                    in_deg[j] += 1;
                }
            }
        }

        let fill_top = self.color("loomforge.canvas.card_fill_top", [0.10, 0.12, 0.15, 1.0]);
        let fill_bot = self.color("loomforge.canvas.card_fill_bot", [0.07, 0.08, 0.11, 1.0]);
        let border = self.color("loomforge.canvas.card_border", [0.17, 0.19, 0.24, 1.0]);
        let border_sel = self.color("loomforge.canvas.card_border_sel", [0.23, 0.35, 0.63, 1.0]);
        let label = self.color("loomforge.canvas.card_label", [0.91, 0.88, 0.82, 1.0]);
        let label_sel = self.color("loomforge.canvas.card_label_sel", [0.62, 0.72, 1.0, 1.0]);
        let meta = self.color("loomforge.canvas.card_meta", [0.56, 0.54, 0.49, 1.0]);
        let stage_top = self.color("loomforge.canvas.stage_top", [0.11, 0.125, 0.161, 1.0]);
        let stage_bot = self.color("loomforge.canvas.stage_bot", [0.03, 0.035, 0.047, 1.0]);
        let stage_border = self.color("loomforge.canvas.stage_border", [0.15, 0.17, 0.21, 1.0]);
        // The doll's image lands here via the frame graph; this is only its backdrop, so
        // the card reads correctly even before a poster has been rendered. Everything on
        // a card scales with the zoom, text included.
        let z = self.view.zoom;
        let text_x = canvas::card_text_x(z);

        for (i, st) in doc.states().iter().enumerate() {
            let Some(&c) = self.cards.get(i) else {
                continue;
            };
            let sel = self.selected_is(i);
            r.draw_ui_panel(
                c.pos,
                c.size,
                fill_top,
                fill_bot,
                1.0,
                5.0 * z,
                if sel { 2.0 } else { 1.0 },
                if sel { border_sel } else { border },
                0.0,
            );
            let doll = canvas::card_stage_rect(c, z);
            r.draw_ui_panel(
                doll.pos,
                doll.size,
                stage_top,
                stage_bot,
                1.0,
                4.0 * z,
                1.0,
                stage_border,
                0.0,
            );
            r.draw_text(
                &st.name,
                c.pos + Vec2::new(text_x, 12.0 * z),
                17.0 * z,
                if sel { label_sel } else { label },
            );
            let io = format!("IN {} · OUT {}", in_deg[i], st.transitions.len());
            r.draw_text(&io, c.pos + Vec2::new(text_x, 38.0 * z), 11.0 * z, meta);
            r.draw_text(
                &st.clip,
                c.pos + Vec2::new(text_x, 56.0 * z),
                12.0 * z,
                meta,
            );
        }

        // Rubber-band for an in-flight Link drag, drawn last so it rides over the cards.
        if let Some(from) = self.link_from.and_then(|i| self.cards.get(i)) {
            let hint = self.color("loomforge.canvas.drop_hint", [0.435, 0.592, 1.0, 1.0]);
            draw_line(r, from.center(), self.cursor, 2.0, hint);
        }
    }

    /// Draw the TAE timeline into the walker-reserved `lf_tae_strip` rect: header,
    /// frame ruler, the nine lanes, the selected state's events, and the playhead.
    ///
    /// The playhead runs off the SAME clock as the Stage dolls, so the bar sweeping the
    /// timeline and the pose on the selected card are showing the same instant.
    fn draw_tae(&self, r: &mut Renderer, rect: StageRect) {
        let top = self.color("loomforge.tae.fill_top", [0.055, 0.063, 0.086, 1.0]);
        let bot = self.color("loomforge.tae.fill_bot", [0.031, 0.035, 0.047, 1.0]);
        let frame_c = self.color("loomforge.tae.border", [0.431, 0.353, 0.204, 0.35]);
        r.draw_ui_panel(rect.pos, rect.size, top, bot, 1.0, 0.0, 1.0, frame_c, 0.0);

        let title_c = self.color("loomforge.rail_title.color", [0.722, 0.592, 0.353, 1.0]);
        let text_c = self.color("loomforge.rail_text.color", [0.871, 0.847, 0.788, 1.0]);
        r.draw_text(
            &strings::resolve("$lf_tae_timeline"),
            rect.pos + Vec2::new(12.0, 4.0),
            12.0,
            title_c,
        );
        r.draw_text(
            &self.tae_summary(),
            rect.pos + Vec2::new(tae::GUTTER_W + 12.0, 4.0),
            12.0,
            text_c,
        );

        self.draw_tae_strip(r, strip_of(rect));
    }

    /// Draw the lane strip itself — ruler, seven tracks + labels, the selected state's
    /// event bars, root motion, the selected-event highlight, and the playhead. Shared by
    /// the State Machine preview strip and the full-height TAE Editor page, so the two can
    /// never drift in how a window or a point event is drawn.
    fn draw_tae_strip(&self, r: &mut Renderer, strip: tae::Strip) {
        // The frame axis comes from the SELECTED state's clip; with nothing selected the
        // lanes still draw, empty, so the strip never collapses to a blank bar.
        let selected = self
            .doc
            .as_ref()
            .and_then(|d| d.selected().and_then(|i| d.states().get(i)).map(|s| (d, s)));
        let (frames, rate) = selected
            .and_then(|(d, s)| d.clip_axis(&s.clip))
            .unwrap_or((0, 60));

        // Ruler.
        let ruler_c = self.color("loomforge.tae_lane.ruler", [0.561, 0.541, 0.49, 1.0]);
        let track_border = self.color(
            "loomforge.tae_lane.track_border",
            [0.149, 0.169, 0.208, 1.0],
        );
        for f in tae::ruler_ticks(frames) {
            let x = strip.frame_x(f, frames);
            r.draw_text(
                &f.to_string(),
                Vec2::new(x + 3.0, strip.pos.y),
                10.0,
                ruler_c,
            );
            draw_line(
                r,
                Vec2::new(x, strip.pos.y + tae::RULER_H - 4.0),
                Vec2::new(x, strip.pos.y + tae::RULER_H),
                1.0,
                track_border,
            );
        }

        // Lane tracks + gutter labels.
        for (i, lane) in tae::Lane::ALL.iter().enumerate() {
            let rect = strip.lane_rect(i);
            let row = self.lane_color(*lane, "row", [0.078, 0.09, 0.122, 1.0]);
            let row_border = self.lane_color(*lane, "row_border", [0.169, 0.188, 0.235, 1.0]);
            let swatch = self.lane_color(*lane, "swatch", [0.561, 0.541, 0.49, 1.0]);
            r.draw_ui_panel(
                rect.pos, rect.size, row, row, 0.0, 3.0, 1.0, row_border, 0.0,
            );
            // The design's 9px lane chip, then the label beside it.
            let chip = (rect.size.y * 0.4).clamp(4.0, 9.0);
            let cy = rect.pos.y + (rect.size.y - chip) * 0.5;
            r.draw_ui_panel(
                Vec2::new(strip.pos.x + 8.0, cy),
                Vec2::splat(chip),
                swatch,
                swatch,
                0.0,
                1.0,
                0.0,
                [0.0; 4],
                0.0,
            );
            r.draw_text(
                &strings::resolve(lane_token(*lane)),
                Vec2::new(strip.pos.x + 8.0 + chip + 6.0, cy - 1.0),
                10.0,
                swatch,
            );
        }

        // Events of the selected state, each on its mapped lane.
        let Some((_, state)) = selected else { return };
        let sel_state = self.doc.as_ref().and_then(|d| d.selected());
        let lane_index = |l: tae::Lane| tae::Lane::ALL.iter().position(|x| *x == l).unwrap_or(0);
        let border_c = self.color("loomforge.tae_lane.event_sel", [0.435, 0.592, 1.0, 1.0]);
        for (idx, ev) in state.events.iter().enumerate() {
            let lane = tae::lane_of(ev.kind);
            let i = lane_index(lane);
            let bar = strip.event_rect(i, ev.tick, ev.end, frames);
            let fill = self.lane_color(lane, "event", [0.561, 0.541, 0.49, 1.0]);
            r.draw_ui_panel(bar.pos, bar.size, fill, fill, 0.0, 2.0, 0.0, [0.0; 4], 0.0);
            // The selected event gets the design's rune-blue outline.
            let is_sel = self.tae_event
                == sel_state.map(|s| doc::EventRef {
                    state: s,
                    index: idx,
                });
            if is_sel {
                draw_rect_outline(r, bar.pos, bar.size, 1.0, border_c);
            }
        }
        // NOTE: root motion is deliberately NOT drawn here. It is `StateDef.root_motion`, a
        // per-state bool — a state-shaped fact, which the authoring contract keeps off the
        // timeline so it has exactly one source of truth. It surfaces in the state
        // inspector instead.

        // Playhead, on the dolls' clock so the bar and the pose agree.
        if frames > 0 {
            let head = self.tae_playhead(frames, rate);
            let x = strip.frame_x(head, frames);
            let c = self.color("loomforge.tae_lane.playhead", [0.435, 0.592, 1.0, 1.0]);
            draw_line(
                r,
                Vec2::new(x, strip.pos.y),
                Vec2::new(x, strip.pos.y + strip.size.y),
                2.0,
                c,
            );
        }
    }

    /// The TAE Editor page's full-height timeline: the same lane strip as the preview,
    /// drawn into the walker-reserved `lf_tae_page_strip` rect and pickable (click an
    /// event bar to inspect it).
    fn draw_tae_page(&self, r: &mut Renderer, rect: StageRect) {
        let top = self.color("loomforge.tae.fill_top", [0.055, 0.063, 0.086, 1.0]);
        let bot = self.color("loomforge.tae.fill_bot", [0.031, 0.035, 0.047, 1.0]);
        let frame_c = self.color("loomforge.tae.border", [0.431, 0.353, 0.204, 0.35]);
        r.draw_ui_panel(rect.pos, rect.size, top, bot, 1.0, 0.0, 1.0, frame_c, 0.0);
        let title_c = self.color("loomforge.rail_title.color", [0.722, 0.592, 0.353, 1.0]);
        r.draw_text(
            &strings::resolve("$lf_timeline"),
            rect.pos + Vec2::new(12.0, 4.0),
            12.0,
            title_c,
        );
        self.draw_tae_strip(r, strip_of(rect));
    }

    /// Pick the event under the cursor on the full-page timeline. Returns the event to
    /// select — a bar hit within its lane, nearest-first so overlapping windows resolve to
    /// the closest edge. Point events get the grab radius so a 5px marker is still clickable.
    fn tae_pick_event(&self, cursor: Vec2) -> Option<doc::EventRef> {
        let doc = self.doc.as_ref()?;
        let state = doc.selected()?;
        let st = doc.states().get(state)?;
        let (frames, _) = doc.clip_axis(&st.clip)?;
        let strip = strip_of(self.page_strip_rect?);
        let lane_index = |l: tae::Lane| tae::Lane::ALL.iter().position(|x| *x == l).unwrap_or(0);
        st.events
            .iter()
            .enumerate()
            .filter_map(|(idx, ev)| {
                let i = lane_index(tae::lane_of(ev.kind));
                let bar = strip.event_rect(i, ev.tick, ev.end, frames);
                let within_y = cursor.y >= bar.pos.y && cursor.y <= bar.pos.y + bar.size.y;
                let x0 = bar.pos.x - TAE_GRAB;
                let x1 = bar.pos.x + bar.size.x + TAE_GRAB;
                if within_y && cursor.x >= x0 && cursor.x <= x1 {
                    let dist = (cursor.x - (bar.pos.x + bar.size.x * 0.5)).abs();
                    Some((doc::EventRef { state, index: idx }, dist))
                } else {
                    None
                }
            })
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(e, _)| e)
    }

    /// One of a lane's four colours, read without building a path string per lookup —
    /// this runs seven times a frame for three keys apiece.
    fn lane_color(&self, lane: tae::Lane, key: &str, fallback: [f32; 4]) -> [f32; 4] {
        let v = self
            .ui_styles
            .get("loomforge")
            .and_then(|v| v.get("tae_lane"))
            .and_then(|v| v.get(lane.id()))
            .and_then(|v| v.get(key));
        json_rgba(v, fallback)
    }

    /// Read an rgba from the resolved `ui_theme.json` by dotted path. The canvas is
    /// scene-drawn, so it can't ride a node `style` — but the colours still come from
    /// the ONE palette, never a private constant. `fallback` covers a missing path.
    fn color(&self, path: &str, fallback: [f32; 4]) -> [f32; 4] {
        let mut cur = &self.ui_styles;
        for seg in path.split('.') {
            match cur.get(seg) {
                Some(v) => cur = v,
                None => return fallback,
            }
        }
        json_rgba(Some(cur), fallback)
    }
}

/// An rgba out of an already token-resolved colour array (`resolve_tokens` has turned
/// `$bronze` into four floats by the time the styles reach here). Anything else — a
/// missing key, an unresolved `$token` still a string — falls back rather than throwing,
/// because these are authored paths and a typo must not take the frame down.
fn json_rgba(v: Option<&serde_json::Value>, fallback: [f32; 4]) -> [f32; 4] {
    match v.and_then(|v| v.as_array()) {
        Some(a) if a.len() >= 4 => {
            let mut out = fallback;
            for (i, c) in a.iter().take(4).enumerate() {
                out[i] = c.as_f64().unwrap_or(fallback[i] as f64) as f32;
            }
            out
        }
        _ => fallback,
    }
}

/// Draw a 2D line as a thin quad. The HUD has no line/curve command, and the canvas is
/// scene-drawn, so a transition edge is two triangles.
/// A hollow rectangle outline — four lines. Used for the selected event's highlight, which
/// must not cover the bar's own fill the way a filled panel would.
fn draw_rect_outline(r: &mut Renderer, pos: Vec2, size: Vec2, width: f32, color: [f32; 4]) {
    let tr = Vec2::new(pos.x + size.x, pos.y);
    let bl = Vec2::new(pos.x, pos.y + size.y);
    let br = pos + size;
    draw_line(r, pos, tr, width, color);
    draw_line(r, tr, br, width, color);
    draw_line(r, br, bl, width, color);
    draw_line(r, bl, pos, width, color);
}

fn draw_line(r: &mut Renderer, a: Vec2, b: Vec2, width: f32, color: [f32; 4]) {
    let d = b - a;
    let len = d.length();
    if len < 0.001 {
        return;
    }
    let n = Vec2::new(-d.y, d.x) / len * (width * 0.5);
    r.draw_triangle(a + n, b + n, b - n, color);
    r.draw_triangle(a + n, b - n, a - n, color);
}

/// Build the Loomforge Bench as a boxed [`Scene`] — the CLIENT BEHAVIOUR the roster
/// registers; the manifest resolves `loomforge.scene.json` and hands its def here.
pub fn scene(def: &SceneDef) -> Box<dyn Scene> {
    Box::new(LoomforgeBench::new(def))
}

// ── small UiNode builders ────────────────────────────────────────────────────

fn node(component: &str) -> UiNode {
    UiNode {
        component: component.to_string(),
        ..Default::default()
    }
}

fn prop(mut n: UiNode, key: &str, value: Value) -> UiNode {
    n.props.insert(key.to_string(), value);
    n
}

fn text_val(s: impl Into<String>) -> Value {
    Value::Text(s.into())
}

/// The `$token` naming a pack kind's display label.
fn kind_token(k: packs::PackKind) -> &'static str {
    match k {
        packs::PackKind::Locomotion => "$lf_kind_locomotion",
        packs::PackKind::Weapon => "$lf_kind_weapon",
        packs::PackKind::Ability => "$lf_kind_ability",
        packs::PackKind::Creature => "$lf_kind_creature",
    }
}

/// The `$token` naming a timeline lane's gutter label.
fn lane_token(l: tae::Lane) -> &'static str {
    match l {
        tae::Lane::Hitbox => "$lf_lane_hitbox",
        tae::Lane::IFrame => "$lf_lane_iframe",
        tae::Lane::Parry => "$lf_lane_parry",
        tae::Lane::Cancel => "$lf_lane_cancel",
        tae::Lane::HyperArmor => "$lf_lane_hyperarmor",
        tae::Lane::Telegraph => "$lf_lane_telegraph",
        tae::Lane::Sfx => "$lf_lane_sfx",
        tae::Lane::Vfx => "$lf_lane_vfx",
        tae::Lane::Notify => "$lf_lane_notify",
    }
}

/// One refilled clip-library row: a live doll dancing the clip, plus its name.
///
/// The WHOLE ROW is the drag source, so the doll is the handle Aaron reaches for —
/// "the doll is animating its script and I drag it into the target card". Drag pickup
/// is prop-driven in the walker; the label rides a per-row BIND so no literal enters
/// the tree, and the doll's content-keyed id stays the honest poster-cache key.
fn clip_row(row_i: usize, name: &str) -> UiNode {
    let mut row = node("row");
    row.id = format!("clip_{name}");
    row.size = Some(CLIP_ROW_H);
    row.pad = 3.0;
    row.gap = 8.0;
    row = prop(row, "style", text_val("loomforge.clip_row"));
    row = prop(row, "drag_kind", text_val("clip"));
    row = prop(row, "drag_id", text_val(name));

    let mut doll = node("surface");
    doll.id = stage_id(name);
    doll.size = Some(CLIP_STAGE);
    doll = prop(doll, "style", text_val("loomforge.clip_stage"));
    doll = prop(doll, "source", text_val(DOLL_SOURCE));
    // Liveness is authored, not hard-coded: the scene publishes this key from the
    // pointer's position, so only the row under the cursor spends a GPU submit.
    doll = prop(doll, "live_bind", text_val(live_key(name)));

    let mut label = node("text");
    label.grow = Some(1.0);
    label = prop(label, "text_bind", text_val(format!("clipname_{row_i}")));
    label = prop(label, "text_size", Value::Number(13.0));
    label = prop(label, "color", text_val("loomforge.clip_row.label"));

    row.children = vec![doll, label];
    row
}

/// One refilled pack card: a Stage doll over the pack's bound name and kind line.
fn pack_card(idx: usize) -> UiNode {
    let mut card = node("cell");
    card.id = format!("packcard_{idx}");
    card.action = Some(format!("packcard_{idx}"));
    card.width = Some(PACK_CARD_W);
    card.height = Some(PACK_CARD_H);
    card.pad = 8.0;
    card.gap = 3.0;
    card.tab_group = "lf_pack_grid".to_string();
    card.nav_ordinal = idx as u32 + 1;
    card = prop(card, "style_bind", text_val(format!("packcard_{idx}_sty")));

    let mut thumb = node("surface");
    thumb.id = format!("{PACK_STAGE_PREFIX}{idx}");
    thumb.width = Some(PACK_STAGE);
    thumb.height = Some(PACK_STAGE);
    thumb = prop(thumb, "style", text_val("loomforge.clip_stage"));
    thumb = prop(thumb, "source", text_val(DOLL_SOURCE));

    let mut name = node("text");
    name = prop(name, "text_bind", text_val(format!("packname_{idx}")));
    name = prop(name, "text_size", Value::Number(15.0));
    name = prop(name, "color", text_val("loomforge.body_text.color"));

    let mut meta = node("text");
    meta = prop(meta, "text_bind", text_val(format!("packmeta_{idx}")));
    meta = prop(meta, "text_size", Value::Number(11.0));
    meta = prop(
        meta,
        "color_bind",
        text_val(format!("packmeta_{idx}_color")),
    );

    card.children = vec![thumb, name, meta];
    card
}

/// A clip-row doll's node id. It doubles as the stage cache key, so it must name
/// everything the rendered image depends on — for a clip row, that is the clip.
fn stage_id(clip: &str) -> String {
    format!("{STAGE_PREFIX}{clip}")
}

/// The Model key a clip-row doll's `live_bind` reads.
fn live_key(clip: &str) -> String {
    format!("live_{clip}")
}

/// The lane strip inside a walker-reserved timeline rect: the header line is drawn
/// above it, so the strip proper starts `TAE_HEADER_H` down. ONE derivation, shared
/// by drawing and picking, so the two can never disagree.
fn strip_of(rect: StageRect) -> tae::Strip {
    tae::Strip {
        pos: rect.pos + Vec2::new(0.0, TAE_HEADER_H),
        size: Vec2::new(rect.size.x, (rect.size.y - TAE_HEADER_H).max(1.0)),
    }
}

fn rect_contains(r: &StageRect, p: Vec2) -> bool {
    p.x >= r.pos.x && p.x <= r.pos.x + r.size.x && p.y >= r.pos.y && p.y <= r.pos.y + r.size.y
}

/// A `text` node: `color` is a dotted path into the resolved `ui_theme.json`.
/// The wire name of a response — the exact value written into the mask.
fn response_label(r: Response) -> &'static str {
    match r {
        Response::Block => "block",
        Response::Parry => "parry",
        Response::Dodge => "dodge",
        Response::Jump => "jump",
        Response::Counter => "counter",
    }
}

/// The wire name of a hit type — the exact value written back into the pack.
fn hit_type_label(h: flicker_skeletal::state::HitType) -> &'static str {
    use flicker_skeletal::state::HitType;
    match h {
        HitType::Slash => "slash",
        HitType::Thrust => "thrust",
        HitType::Strike => "strike",
        HitType::Sweep => "sweep",
        HitType::Grab => "grab",
    }
}

/// Add `v` to a filter set, or remove it if already there — the set-toggle every filter
/// row uses, so "clicking the active row clears it" needs no per-call branch.
fn toggle<T: PartialEq>(set: &mut Vec<T>, v: T) {
    if let Some(i) = set.iter().position(|x| *x == v) {
        set.remove(i);
    } else {
        set.push(v);
    }
}

/// Greedy word wrap — the HUD has no text-measure at tree-build time, so authored notes are
/// wrapped by character budget. A word longer than the budget gets its own line rather than
/// being split, which keeps path-like tokens readable.
fn wrap_text(s: &str, cols: usize) -> Vec<String> {
    let cols = cols.max(1);
    let mut out: Vec<String> = Vec::new();
    let mut line = String::new();
    for word in s.split_whitespace() {
        if line.is_empty() {
            line.push_str(word);
        } else if line.chars().count() + 1 + word.chars().count() <= cols {
            line.push(' ');
            line.push_str(word);
        } else {
            out.push(std::mem::take(&mut line));
            line.push_str(word);
        }
    }
    if !line.is_empty() {
        out.push(line);
    }
    out
}

/// Last two path components — enough to identify a pack without the full path.
fn short_path(p: &Path) -> String {
    let file = p
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    match p.parent().and_then(|d| d.file_name()) {
        Some(dir) => format!("{}/{}", dir.to_string_lossy(), file),
        None => file,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Load the shipped stringtable (en-us) into the process-wide table, so tests
    /// asserting composed copy read FINAL text. Safe across parallel test threads —
    /// every caller loads the same content.
    fn load_shipped_strings() {
        let strings = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../content/data/stringtable.json"
        ))
        .expect("stringtable reads");
        flicker::ui::strings::load_str(&strings, "en-us");
    }

    /// The MODEL-CHANNEL strings gate: display copy published from Rust into the
    /// Model bypasses the tree-walking strings gate, so the crate self-gates its
    /// own source — every `.set`/`.with` value must be a resolved `$token`, a data
    /// shape, or carry an explicit `strings-gate-exempt` reason.
    #[test]
    fn no_raw_display_copy_published_into_the_model() {
        let flags = flicker::ui::strings::raw_model_publish_literals(include_str!("lib.rs"));
        assert!(
            flags.is_empty(),
            "raw display copy published into the Model: {flags:?}"
        );
    }

    /// The tab set + their ids are what the authored tree and the action router agree
    /// on — a mismatch would silently break tab switching.
    #[test]
    fn tabs_have_unique_ids_and_labels() {
        let mut ids: Vec<&str> = Tab::ALL.iter().map(|t| t.id()).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), Tab::ALL.len(), "tab ids must be unique");
        assert!(Tab::ALL.iter().all(|t| !t.label().is_empty()));
        assert_eq!(
            Tab::default(),
            Tab::StateMachine,
            "the bench opens on the graph"
        );
    }

    /// The shipped scene file IS the bench: it parses, names this behaviour, authors
    /// a tree whose root declares the pause + cycle intents, and carries the three
    /// refill container cells the behaviour fills at event time.
    #[test]
    fn the_shipped_scene_file_authors_the_bench() {
        let def = SceneDef::parse("loomforge", LF_SCENE).expect("scene file parses");
        assert_eq!(def.behaviour, "loomforge");
        let tree = def.tree.expect("the scene file carries the chrome tree");
        let intents = UiIntents::of(&tree);
        use flicker_input_core::ActionSignal;
        assert_eq!(intents.result_for(ActionSignal::Menu), Some("pause_open"));
        let mut t = tree.clone();
        for container in ["lf_clip_rows", "lf_pack_cards", "lf_skel_rows"] {
            assert!(
                find_by_id_mut(&mut t, container).is_some(),
                "the `{container}` refill container is authored"
            );
        }
        // The tab buttons fire the SAME ids the dispatcher routes.
        let mut ids: Vec<String> = Vec::new();
        collect_ids(&tree, &mut ids);
        for t in Tab::ALL {
            assert!(ids.iter().any(|i| i == t.id()), "{} authored", t.id());
        }
        for t in Tool::ALL {
            assert!(ids.iter().any(|i| i == t.id()), "{} authored", t.id());
        }
        for b in [
            "save",
            "validate",
            "cycle_trigger",
            "edge_delete",
            "pack_load",
            "tae_play",
            "tae_prev",
            "tae_next",
            TAE_STAGE_ID,
            "lf_canvas",
            "lf_tae_strip",
            "lf_tae_page_strip",
        ] {
            assert!(ids.iter().any(|i| i == b), "{b} authored");
        }
    }

    /// THE PAIR-SCRIPT REGRESSION GATE: build the bench exactly as the resolver does
    /// (real def, real loomforge.lua) and run the REAL model path — the raw cursors
    /// must come back as the DERIVED page gates and washes the tree binds.
    #[test]
    fn the_pair_script_derives_the_page_gates_and_washes() {
        load_shipped_strings();
        let mut ed = LoomforgeBench::shipped();
        assert!(ed.script.is_some(), "loomforge.lua loads (the pair script)");
        let m = ed.model();
        assert!(m.is_on("page_sm"), "the bench opens on the graph page");
        for gate in ["page_pack", "page_creature", "page_tae"] {
            assert!(!m.is_on(gate), "{gate} is off at open");
        }
        assert_eq!(m.text("tab_sm_sty"), Some("loomforge.tab_active"));
        assert_eq!(m.text("tab_tae_sty"), Some("loomforge.tab_idle"));
        assert_eq!(m.text("tool_select_sty"), Some("loomforge.tab_active"));
        assert!(m.is_on("rail_clips"), "no edge selected → the clip library");
        assert!(!m.is_on("rail_edge"));

        // Switch pages through the dispatcher and the derived gates follow.
        ed.apply_actions(&ValueMap::new().with("tab_tae", true));
        let m = ed.model();
        assert!(m.is_on("page_tae") && !m.is_on("page_sm"));
        assert_eq!(m.text("tab_tae_sty"), Some("loomforge.tab_active"));
    }

    /// Walk the REAL tree with the REAL derived model and gate the authored data:
    /// known kinds only, no raw display literals, and the chrome actually draws
    /// with real extents for the must-hit controls.
    #[test]
    fn hud_tree_walks_with_model() {
        load_shipped_strings();
        let def = SceneDef::parse("loomforge", LF_SCENE).expect("scene file parses");
        let tree = def.tree.clone().expect("scene defines a tree");
        let styles = flicker::ui::load_shared_styles(def.styles.as_ref());

        assert!(
            flicker::ui::unknown_kinds(&tree).is_empty(),
            "loomforge.scene.json names unknown kinds: {:?}",
            flicker::ui::unknown_kinds(&tree)
        );
        assert!(
            flicker::ui::raw_display_literals(&tree).is_empty(),
            "loomforge.scene.json ships raw display literals: {:?}",
            flicker::ui::raw_display_literals(&tree)
        );

        let mut ed = LoomforgeBench::shipped();
        let m = ed.model();
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
        assert!(!frame.commands.is_empty(), "the chrome draws");
        let has_text = |needle: &str| {
            frame
                .commands
                .iter()
                .any(|c| matches!(c, HudCommand::Text { text, .. } if text.contains(needle)))
        };
        assert!(has_text("Loomforge Bench"), "the title renders");
        assert!(has_text("State Machine"), "the tab labels render");
        assert!(has_text("PACK MANAGER"), "the SM rail renders");
        // Presence AND extent for the controls a user must be able to hit — a
        // control can be well-formed in the tree and still resolve to zero pixels.
        for id in ["save", "validate", "tab_sm", "tab_tae", "lf_canvas"] {
            let rect = frame
                .rects
                .iter()
                .find(|(i, _)| i == id)
                .map(|(_, r)| *r)
                .unwrap_or_else(|| panic!("{id} resolved no rect"));
            assert!(
                rect[2] > 1.0 && rect[3] > 1.0,
                "{id} resolved to zero extent: {rect:?}"
            );
        }
    }

    /// The declared pause intent through the scene's real chain — the authored
    /// root's `on_menu` reaches the walker, which consumes the Menu press and fires
    /// the name the ONE dispatch maps onto the pause push (the re-pointed half of
    /// the retired route.rs tests).
    #[test]
    fn the_declared_pause_intent_fires_through_the_authored_tree() {
        use flicker_input_core::{ActionSignal, EventKind, InputContext};
        use flicker_input_router::{InputEvent, RouteCtx};

        let def = SceneDef::parse("loomforge", LF_SCENE).expect("scene file parses");
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
        let mut rc = RouteCtx::new();
        let report = {
            let mut chain: [&mut dyn InputHandler; 1] = [&mut walker];
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

    /// Clicking a tab button routes to that page — and the declared cycle intents
    /// walk the ring with wrap.
    #[test]
    fn tab_actions_switch_the_page() {
        let mut bench = LoomforgeBench::shipped();
        let mut results = ValueMap::new();
        results.set(Tab::TaeEditor.id(), true);
        bench.apply_actions(&results);
        assert_eq!(bench.tab(), Tab::TaeEditor);

        // The bumper ring: TAE is the last page, so next wraps to the graph.
        bench.apply_actions(&ValueMap::new().with("tab_next", true));
        assert_eq!(bench.tab(), Tab::StateMachine, "tab_next wraps");
        bench.apply_actions(&ValueMap::new().with("tab_prev", true));
        assert_eq!(bench.tab(), Tab::TaeEditor, "tab_prev wraps back");
    }

    /// Tool selection + trigger cycling route through the same action map as the
    /// tabs — and the declared mode-cycle intents walk the tool ring.
    #[test]
    fn tool_and_trigger_actions_route() {
        let mut bench = LoomforgeBench::shipped();
        assert_eq!(bench.tool, Tool::Select, "Select is the default tool");

        let mut r = ValueMap::new();
        r.set(Tool::Link.id(), true);
        bench.apply_actions(&r);
        assert_eq!(bench.tool, Tool::Link);

        let before = bench.link_trigger;
        let mut r = ValueMap::new();
        r.set("cycle_trigger", true);
        bench.apply_actions(&r);
        assert_ne!(bench.link_trigger, before, "cycling advances the trigger");

        // Switching tools mid-drag abandons the half-drawn edge rather than leaving a
        // stale source that would link to whatever is clicked next.
        bench.link_from = Some(0);
        let mut r = ValueMap::new();
        r.set(Tool::Select.id(), true);
        bench.apply_actions(&r);
        assert_eq!(bench.tool, Tool::Select);
        assert!(
            bench.link_from.is_none(),
            "switching tools abandons the pending edge"
        );

        // The declared mode-cycle intents walk the ring (and also drop a pending edge).
        bench.link_from = Some(0);
        bench.apply_actions(&ValueMap::new().with("tool_next", true));
        assert_eq!(bench.tool, Tool::AddState, "tool_next steps the ring");
        assert!(bench.link_from.is_none());
        bench.apply_actions(&ValueMap::new().with("tool_prev", true));
        assert_eq!(bench.tool, Tool::Select, "tool_prev steps back");
    }

    /// The Phase-3 deliverable end to end, against the REAL pack: load → bind a clip
    /// (the drag-drop edit) → save → reload, with the `_note` header surviving.
    #[test]
    fn load_bind_save_round_trips_to_disk() {
        let pack_path = pack_path();
        if !flicker_core::compression::file_exists(&pack_path) {
            return; // content not present in this checkout — nothing to assert
        }
        let mut doc = EditorDoc::load(&pack_path, &[&base_dir(), &clips_dir()])
            .expect("the shipped pack + clip library must load");
        assert!(!doc.states().is_empty(), "pack has states");

        // A bad drop is rejected and must NOT dirty the document.
        assert!(!doc.bind_clip(0, "definitely_not_a_real_clip"));
        assert!(!doc.dirty(), "a rejected drop must not dirty the document");
        assert!(
            !doc.bind_clip(usize::MAX, "whatever"),
            "unknown state rejected too"
        );

        // Bind a clip the first state is not already using.
        let current = doc.states()[0].clip.clone();
        let clip = doc
            .clip_names()
            .find(|c| *c != current)
            .expect("library has a second clip")
            .to_string();
        assert!(doc.bind_clip(0, &clip), "binding a real clip succeeds");
        assert!(doc.dirty(), "a real edit dirties the document");
        assert_eq!(doc.states()[0].clip, clip);

        // Write to a temp copy (never the repo asset) and reload it.
        let tmp = std::env::temp_dir().join("loomforge_round_trip.pack.json");
        let note = doc.pack().note.clone();
        flicker_skeletal::state::write_pack(&tmp, doc.pack()).expect("write");
        let reloaded = flicker_skeletal::state::read_pack(&tmp).expect("read back");
        assert_eq!(
            reloaded.state_machine.states[0].clip, clip,
            "the edit persisted"
        );
        assert_eq!(
            reloaded.note, note,
            "the hand-authored _note survived the save"
        );
        assert_eq!(
            reloaded.state_machine.states.len(),
            doc.states().len(),
            "no states lost in the round trip"
        );
        let _ = std::fs::remove_file(&tmp);
        // The save emits the gz-at-rest form at the logical path's `.gz` twin.
        let _ = std::fs::remove_file(flicker_core::compression::gz_sibling(&tmp));
    }

    /// A refilled clip row is the drag handle AND carries the doll. The `surface` node
    /// must name a source that actually exists in the shared `stages` block, and
    /// bind its liveness to the key the scene publishes — a typo in either silently
    /// costs a GPU submit per row, or a doll that never animates.
    #[test]
    fn clip_row_carries_a_bound_doll_and_is_a_drag_source() {
        let row = clip_row(0, "walk_forward");

        assert_eq!(row.props.get("drag_kind"), Some(&text_val("clip")));
        assert_eq!(row.props.get("drag_id"), Some(&text_val("walk_forward")));

        let doll = row
            .children
            .iter()
            .find(|c| c.component == "surface")
            .expect("the row carries a doll");
        assert_eq!(doll.props.get("source"), Some(&text_val(DOLL_SOURCE)));
        assert_eq!(
            doll.props.get("live_bind"),
            Some(&text_val(live_key("walk_forward"))),
            "liveness must bind to the key the scene publishes"
        );
        assert!(
            doll.props.contains_key("style"),
            "the doll has a backdrop frame"
        );

        // The id is the stage cache key; the scene reads the clip name back off it.
        assert_eq!(doll.id, stage_id("walk_forward"));
        assert_eq!(doll.id.strip_prefix(STAGE_PREFIX), Some("walk_forward"));

        // The label rides a per-row BIND — no literal enters the refilled tree.
        let label = row
            .children
            .iter()
            .find(|c| c.component == "text")
            .expect("the row carries its name");
        assert_eq!(label.props.get("text_bind"), Some(&text_val("clipname_0")));

        // And the source it names must be one the shared JSON actually defines.
        let def = SceneDef::parse("loomforge", LF_SCENE).expect("scene file parses");
        let styles = flicker::ui::load_shared_styles(def.styles.as_ref());
        assert!(
            stage::parse_sources(&styles).contains_key(DOLL_SOURCE),
            "`{DOLL_SOURCE}` must exist in the shared `stages` block"
        );
    }

    /// Walk a built tree, collecting every non-empty node id.
    fn collect_ids(n: &UiNode, out: &mut Vec<String>) {
        if !n.id.is_empty() {
            out.push(n.id.clone());
        }
        for c in &n.children {
            collect_ids(c, out);
        }
    }

    /// Every pack-kind accent must resolve in the scene file's folded styles. These
    /// are colour paths, which fall back silently, so a rename would show up only as
    /// grey labels in the window — pin them here instead.
    #[test]
    fn every_pack_kind_colour_resolves_in_the_scene_styles() {
        let bench = LoomforgeBench::shipped();
        let miss = [-1.0, -1.0, -1.0, -1.0];
        for k in packs::PackKind::ALL {
            let c = bench.color(k.color_path(), miss);
            assert_ne!(c, miss, "{} is missing from the styles", k.color_path());
            assert!(c.iter().all(|v| v.is_finite()));
        }
    }

    /// The Pack Browser's REFILLED grid must be well-formed over the real library:
    /// unique node ids (duplicate ids would make two cards report the same action)
    /// and a card + doll per visible pack.
    #[test]
    fn pack_browser_refill_is_well_formed_over_the_real_library() {
        let mut bench = LoomforgeBench::shipped();
        bench.packs = packs::scan_packs(&content_characters());
        if bench.packs.is_empty() {
            return; // content tree absent in this checkout
        }
        bench.refill_skel_rows();
        bench.refill_pack_cards();
        let tree = bench.authored.as_ref().expect("authored tree held");

        let mut ids: Vec<String> = Vec::new();
        collect_ids(tree, &mut ids);
        let mut sorted = ids.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(
            sorted.len(),
            ids.len(),
            "duplicate node id after the refill"
        );

        let vis = bench.visible_packs().len();
        assert_eq!(
            ids.iter().filter(|i| i.starts_with("packcard_")).count(),
            vis,
            "one card per visible pack"
        );
        assert_eq!(
            ids.iter()
                .filter(|i| i.starts_with(PACK_STAGE_PREFIX))
                .count(),
            vis,
            "every card carries a Stage doll"
        );
        assert_eq!(
            ids.iter().filter(|i| i.starts_with("packkind_")).count(),
            packs::PackKind::ALL.len(),
            "the four authored kind filters"
        );
        assert_eq!(
            ids.iter().filter(|i| i.starts_with("packskel_")).count(),
            packs::skeletons(&bench.packs).len(),
            "one refilled row per skeleton on disk"
        );
    }

    /// The clip rail's refill windows over the real library and re-windows on scroll.
    #[test]
    fn the_clip_rail_refills_and_scrolls_over_the_real_library() {
        let doc = EditorDoc::load(&pack_path(), &[&base_dir(), &clips_dir()]);
        let Ok(doc) = doc else { return }; // content tree absent
        let total = doc.clip_names().count();
        let mut bench = LoomforgeBench::shipped();
        bench.doc = Some(doc);
        bench.refill_clip_rows();

        let count = |b: &LoomforgeBench| {
            let mut ids = Vec::new();
            collect_ids(b.authored.as_ref().unwrap(), &mut ids);
            ids.iter()
                .filter(|i| i.starts_with(STAGE_PREFIX))
                .cloned()
                .collect::<Vec<_>>()
        };
        let first = count(&bench);
        assert_eq!(
            first.len(),
            total.min(CLIP_ROWS),
            "one doll per visible row"
        );

        if total > CLIP_ROWS {
            bench.scroll_clips(-1.0);
            let scrolled = count(&bench);
            assert_ne!(first, scrolled, "scrolling re-windows the rows");
            assert_eq!(scrolled.len(), CLIP_ROWS);
        }
    }

    /// Every lane-swatch style read by dotted path must resolve — a rename would
    /// show up only as grey lane labels in the window.
    #[test]
    fn every_tae_lane_swatch_resolves() {
        let bench = LoomforgeBench::shipped();
        let miss = [-1.0, -1.0, -1.0, -1.0];
        for lane in tae::Lane::ALL {
            let path = format!("loomforge.tae_lane.{}.swatch", lane.id());
            assert_ne!(bench.color(&path, miss), miss, "{path} missing");
        }
    }

    /// Filtering must never leave the detail pane describing a hidden pack.
    #[test]
    fn filtering_reclamps_the_selection() {
        let mut bench = LoomforgeBench::shipped();
        bench.packs = packs::scan_packs(&content_characters());
        if bench.packs.len() < 2 {
            return;
        }
        bench.pack_sel = bench.visible_packs().len() - 1;
        // Filter down to the first pack's kind only.
        let kind = bench.packs[0].kind;
        let mut results = ValueMap::default();
        let idx = packs::PackKind::ALL
            .iter()
            .position(|k| *k == kind)
            .unwrap();
        results.set(format!("packkind_{idx}"), true);
        bench.apply_pack_actions(&results);

        let vis = bench.visible_packs();
        assert!(!vis.is_empty());
        assert!(
            bench.pack_sel < vis.len(),
            "selection must address a visible card"
        );
        assert!(vis.iter().all(|e| e.kind == kind));
        // The selected entry is one of the visible ones.
        assert!(bench.selected_pack().is_some());
    }

    /// The doll's backdrop styles must resolve to real rgba. `color()` falls back
    /// silently on a missing path, so a renamed key would go unnoticed until someone
    /// looked at the window — exactly the brittleness of scene-drawn dotted lookups.
    #[test]
    fn card_stage_colours_resolve_in_the_scene_styles() {
        let bench = LoomforgeBench::shipped();
        let miss = [-1.0, -1.0, -1.0, -1.0];
        for path in [
            "loomforge.canvas.stage_top",
            "loomforge.canvas.stage_bot",
            "loomforge.canvas.stage_border",
            "loomforge.canvas.card_label",
            "loomforge.canvas.edge",
        ] {
            let c = bench.color(path, miss);
            assert_ne!(c, miss, "{path} is missing from the styles");
            assert!(
                c.iter().all(|v| v.is_finite()),
                "{path} resolved to a non-colour"
            );
        }
        // The clip-row doll's frame is a walker `style`, so it must be a real block.
        assert!(
            bench.ui_styles.pointer("/loomforge/clip_stage").is_some(),
            "loomforge.clip_stage must exist for the clip-row doll"
        );
    }

    /// Every lane's four colours must resolve. `lane_color` falls back silently, so
    /// a renamed key would leave a lane drawn in placeholder grey with nothing
    /// failing — the exact failure mode of scene-drawn dotted-path lookups.
    #[test]
    fn every_tae_lane_resolves_all_four_colours() {
        let bench = LoomforgeBench::shipped();
        let miss = [-1.0, -1.0, -1.0, -1.0];
        for lane in tae::Lane::ALL {
            for key in ["swatch", "row", "row_border", "event"] {
                let c = bench.lane_color(lane, key, miss);
                assert_ne!(c, miss, "loomforge.tae_lane.{}.{key} is missing", lane.id());
                assert!(c.iter().all(|v| v.is_finite()));
            }
        }
        // The strip's shared colours live beside the lanes, not inside one.
        for path in [
            "loomforge.tae_lane.track_border",
            "loomforge.tae_lane.ruler",
            "loomforge.tae_lane.playhead",
            "loomforge.tae.fill_top",
            "loomforge.tae.border",
        ] {
            assert_ne!(bench.color(path, miss), miss, "{path} is missing");
        }
    }

    /// Save/validate with no document must report, not panic — and the report is
    /// token-resolved copy now.
    #[test]
    fn actions_are_safe_without_a_document() {
        load_shipped_strings();
        let mut bench = LoomforgeBench::shipped();
        let mut results = ValueMap::new();
        results.set("save", true);
        bench.apply_actions(&results);
        assert!(bench.status.contains("Nothing to save"));
    }
}
