//! **Loomforge Bench** — the animation-authoring editor scene.
//!
//! A four-tab bench over a `flicker.pack`: State Machine · Pack Browser · Creature
//! Composer · TAE Editor. Unlike the retired `flicker-packeditor` (a read-only viewer),
//! this one WRITES the pack back — see [`EditorDoc::save`].
//!
//! **UI split.** The chrome here (top bar, tab bar, rails, inspectors) is built as a
//! Rust [`UiNode`] tree and run through the `flicker-widgets` component walker. The
//! node-graph canvas and TAE timeline are drawn by the scene itself in later phases —
//! they need free 2D positioning, edges/curves, and drop targets that the walker's
//! closed component set has no templates for.
//!
//! All colours come from the ONE global `ui_elements.json` (`loomforge` section) as
//! `$token` refs into the Prism palette — never a private palette.

mod canvas;
mod doc;
mod packs;
mod route;
mod stage;
mod tae;

pub use doc::{EditorDoc, Tab, Tool};

use doc::{next_trigger, trigger_label, EdgeRef};
use route::RootHandler;
use flicker_skeletal::state::{EventKind, Response, Trigger};

use std::path::Path;
use std::time::Duration;

use canvas::{CanvasArea, CardRect};
use flicker_input_core::{AbstractControls, ContextualBindings, GamepadConfig, InputMap, InputState};
use flicker::render::{FrameGraph, Rect as StageRect, Renderer, TextureHandle, Vec2};
use flicker::scene::{Scene, Transition};
use flicker::script::{
    ComponentLibrary, HudCommand, ScriptHost, UiAnchor, UiNode, Value, ValueMap,
};
use flicker::ui::{
    load_styles, render_hud, run_ui_with, UiInput, UiIntents, UiState, WalkerHandler,
    UI_COMPONENT_MODULES,
};
use flicker_input_core::{Fired, Resolver};
use flicker_input_router::{apply_context_requests, InputEvent, InputHandler, RouteCtx, Router};
use flicker_shell::{PauseScene, Theme};
use stage::{StageReq, StageRig};

/// THE single global UI-element + Prism-palette file.
const HUD_UI_ELEMENTS: &str =
    concat!(env!("CARGO_MANIFEST_DIR"), "/../../Alpha/content/sensorium/resources/ui_elements.json");

/// The pack the bench opens with — Katanami is the rich one (23 states, full TAE:
/// hitbox/cancel windows, footsteps, any-state hit/death edges).
const PACK_PATH: &str =
    concat!(env!("CARGO_MANIFEST_DIR"), "/../../Alpha/content/characters/katanami/Katanami.pack.json");
/// The base body rig, and the clip library the pack's state clips resolve against
/// (same pairing `flicker-packeditor` used).
const BASE_DIR: &str =
    concat!(env!("CARGO_MANIFEST_DIR"), "/../../Alpha/content/characters/PrismHumanBaseA");
const CLIPS_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../Alpha/content/characters/katanami");
/// Root the Pack Browser scans for `*.pack.json` — the character tree.
const CONTENT_CHARACTERS: &str =
    concat!(env!("CARGO_MANIFEST_DIR"), "/../../Alpha/content/characters");

const TOP_BAR_H: f32 = 56.0;
const TAB_BAR_H: f32 = 40.0;
/// Left tool rail (design: 56px).
const TOOL_RAIL_W: f32 = 56.0;
/// The clip-row doll — the design's 34px Stage — and the row that carries it.
const CLIP_STAGE: f32 = 34.0;
const CLIP_ROW_H: f32 = 40.0;
/// Which stage sub-scene the bench's dolls use (a key in the shared `stages` block).
const DOLL_SOURCE: &str = "portrait";
/// Node-id prefix marking a clip-row doll — the scene reads the clip name back off it.
const STAGE_PREFIX: &str = "clipdoll_";
/// How many stranded render targets to tolerate before pruning the stage cache.
const STALE_SLOT_SLACK: usize = 8;
/// Pack Browser card metrics (the design's 3-col grid of thumbed cards).
const PACK_CARD_W: f32 = 150.0;
const PACK_CARD_H: f32 = 150.0;
const PACK_CARD_GAP: f32 = 12.0;
const PACK_GRID_MAX_COLS: usize = 4;
/// The card's Stage doll — the design's 92px thumb.
const PACK_STAGE: f32 = 92.0;
/// Node-id prefix marking a pack-card doll, so the scene can map a slot back to its pack.
const PACK_STAGE_PREFIX: &str = "packdoll_";
/// Characters per wrapped line in the detail pane's note, and how many lines to show.
const NOTE_WRAP: usize = 34;
const NOTE_MAX_LINES: usize = 8;
/// The TAE Editor's preview doll — the design's 300px Stage, capped.
const TAE_STAGE_MAX: f32 = 300.0;
/// Node id of that doll, so the scene can pose it from the edited clip.
const TAE_STAGE_ID: &str = "taedoll";
/// Pointer grab radius for picking an event bar on the timeline, in pixels.
const TAE_GRAB: f32 = 4.0;
/// One press of the capsule stepper, in metres.
const CAPSULE_STEP_M: f32 = 0.05;
/// One press of the parry-window-scale stepper.
const PARRY_SCALE_STEP: f32 = 0.05;
/// The TAE strip's header line, above the ruler and lanes.
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
    /// The screen's declarative signal bindings (S9). Loomforge REBUILDS its tree
    /// every frame (live rects), but the root's `on_<signal>` props are static
    /// data, so the declaration is collected from the first built tree and kept —
    /// the same cache-with-the-tree discipline, at this scene's tree lifetime.
    ui_intents: UiIntents,
    /// Intent names fired last frame — republished ONCE into the next frame's
    /// Model as the transient `sig_<name>` mirror (S9), then dropped.
    fired_sigs: Vec<String>,
    /// Component-library host built once at construction: dispatches `button` DRAW to the
    /// Lua twin (`ui/button.lua`) so the Rust `UiNode` tree renders through the shared
    /// component library instead of the walker's built-in `draw_button`. `None` = all-Rust.
    ui_lib: Option<ScriptHost>,
    hud_commands: Vec<HudCommand>,
    hud_white: Option<TextureHandle>,
    theme: Option<Theme>,

    /// Per-context action maps + the active-context stack. Only the `World` base
    /// (`wasd_and_mouse`) is populated — the editor has no modal contexts — but it
    /// is a `ContextualBindings` so the [`Resolver`] can resolve against it and the
    /// pause overlay reads its `active_map()`.
    bindings: ContextualBindings,
    gamepad_config: GamepadConfig,
    /// The input-router seam (spec §5/§9): a stateful edge [`Resolver`] (replaces the
    /// `menu_prev` bool), a REUSED `Fired` scratch buffer (no per-frame alloc — RT-7),
    /// the router's request queue, and a monotonic frame `tick` that is the resolver's
    /// `TickTime` (NOT wall-clock — spec §3.2a). Menu (Esc → pause) and the walker's
    /// `hud_hit` pointer-consume route through the bus; the bespoke canvas/timeline
    /// tools below stay scene-owned raw-pointer logic.
    resolver: Resolver,
    ev: Vec<Fired>,
    route: RouteCtx,
    tick: u64,

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

/// The TAE Editor page's boxes.
struct TaeRegions {
    screen: Vec2,
    top: f32,
    track_w: f32,
    insp_w: f32,
    strip_y: f32,
    strip_h: f32,
    preview_w: f32,
    preview_h: f32,
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

/// The State Machine page's regions, from the design: tool rail | canvas | pack rail,
/// with the TAE strip across the bottom.
struct Regions {
    screen: Vec2,
    top: f32,
    canvas: CanvasArea,
    rail_x: f32,
    rail_w: f32,
    tae_y: f32,
    tae_h: f32,
}

impl Default for LoomforgeBench {
    fn default() -> Self {
        Self::new()
    }
}

impl LoomforgeBench {
    pub fn new() -> Self {
        Self {
            doc: None,
            load_error: None,
            tab: Tab::default(),
            status: String::new(),
            ui_state: UiState::new(),
            ui_styles: serde_json::Value::Null,
            ui_intents: UiIntents::default(),
            fired_sigs: Vec::new(),
            ui_lib: match ScriptHost::library(UI_COMPONENT_MODULES) {
                Ok(h) => Some(h),
                Err(e) => {
                    tracing::error!("loomforge ui component library failed: {e}");
                    None
                }
            },
            hud_commands: Vec::new(),
            hud_white: None,
            theme: None,
            bindings: ContextualBindings::new(InputMap::wasd_and_mouse()),
            gamepad_config: GamepadConfig::default(),
            resolver: Resolver::new(),
            ev: Vec::new(),
            route: RouteCtx::new(),
            tick: 0,
            cards: Vec::new(),
            canvas_area: CanvasArea { pos: Vec2::ZERO, size: Vec2::ZERO },
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

    /// Split the page below the tab bar into the design's regions. The design's fixed
    /// 560px rail / 190px strip are clamped so the canvas keeps usable width on a
    /// smaller window than the 1920×1080 mock.
    fn regions(screen: Vec2) -> Regions {
        let top = TOP_BAR_H + TAB_BAR_H;
        let rail_w = (screen.x * 0.29).clamp(240.0, 560.0).min(screen.x * 0.5);
        let tae_h = (screen.y * 0.18).clamp(110.0, 190.0);
        let canvas_w = (screen.x - TOOL_RAIL_W - rail_w).max(120.0);
        let canvas_h = (screen.y - top - tae_h).max(120.0);
        Regions {
            screen,
            top,
            canvas: CanvasArea {
                pos: Vec2::new(TOOL_RAIL_W, top),
                size: Vec2::new(canvas_w, canvas_h),
            },
            rail_x: TOOL_RAIL_W + canvas_w,
            rail_w,
            tae_y: top + canvas_h,
            tae_h,
        }
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

    /// Build this frame's chrome tree: top bar, tab bar, and the active page's body.
    /// Rebuilt every frame from the document — the walker is immediate-mode, so there
    /// is no retained widget tree to keep in sync.
    fn build_tree(&self, screen: Vec2) -> UiNode {
        let mut page = node("screen");
        // The screen's input DECLARATION (S9): Menu (Esc / pad Start) fires
        // `pause_open`; `update` maps the fired name onto its pause push. Same
        // prop the Lua-authored screens carry — this root is just built in Rust.
        page.props
            .insert("on_menu".into(), Value::Text("pause_open".into()));
        page.children = vec![
            self.top_bar(screen),
            self.tab_bar(screen),
            self.body(screen),
        ];
        page
    }

    fn top_bar(&self, screen: Vec2) -> UiNode {
        let mut bar = node("row");
        bar.id = "top_bar".into();
        bar.anchor = Some(UiAnchor::TopLeft);
        bar.width = Some(screen.x);
        bar.height = Some(TOP_BAR_H);
        bar.pad = 16.0;
        bar.gap = 12.0;
        bar = prop(bar, "style", text_val("loomforge.top_bar"));

        let title = label_node("Loomforge Bench", 21.0, "loomforge.title.color", Some(210.0));
        let rig = label_node(self.rig_badge(), 11.0, "loomforge.badge.color", Some(240.0));

        // Spacer soaks the middle so the actions sit hard right.
        let mut spacer = node("cell");
        spacer.grow = Some(1.0);

        let mut validate = node("button");
        validate.id = "validate".into();
        validate.action = Some("validate".into());
        validate.size = Some(110.0);
        validate = prop(validate, "label", text_val("Validate Rig"));
        validate = prop(validate, "style", text_val("loomforge.button_secondary"));

        let mut save = node("button");
        save.id = "save".into();
        save.action = Some("save".into());
        save.size = Some(80.0);
        save = prop(save, "label", text_val(if self.is_dirty() { "Save *" } else { "Save" }));
        save = prop(save, "style", text_val("loomforge.button_primary"));

        bar.children = vec![title, rig, spacer, validate, save];
        bar
    }

    fn tab_bar(&self, screen: Vec2) -> UiNode {
        let mut bar = node("row");
        bar.id = "tab_bar".into();
        bar.anchor = Some(UiAnchor::TopLeft);
        bar.offset = [0.0, TOP_BAR_H];
        bar.width = Some(screen.x);
        bar.height = Some(TAB_BAR_H);
        bar.pad = 6.0;
        bar.gap = 4.0;
        bar = prop(bar, "style", text_val("loomforge.tab_bar"));

        bar.children = Tab::ALL
            .iter()
            .map(|t| {
                let mut b = node("button");
                b.id = t.id().into();
                b.action = Some(t.id().into());
                b.size = Some(170.0);
                b = prop(b, "label", text_val(t.label()));
                // The active tab uses the lit style — the design's underline/emphasis.
                let style =
                    if *t == self.tab { "loomforge.tab_active" } else { "loomforge.tab_idle" };
                prop(b, "style", text_val(style))
            })
            .collect();
        bar
    }

    /// The active page's body.
    fn body(&self, screen: Vec2) -> UiNode {
        match self.tab {
            Tab::StateMachine => self.sm_chrome(screen),
            Tab::PackBrowser => self.pack_chrome(screen),
            Tab::TaeEditor => self.tae_chrome(screen),
            _ => self.summary_body(screen),
        }
    }

    /// The State Machine page's walker chrome — tool rail, Pack Manager rail, and the
    /// TAE strip. The node-graph canvas BETWEEN them is drawn by the scene itself
    /// (see `draw_canvas`), which is why nothing here covers that rect.
    fn sm_chrome(&self, screen: Vec2) -> UiNode {
        let r = Self::regions(screen);
        let mut root = node("screen");
        root.id = "sm_page".into();
        root.anchor = Some(UiAnchor::TopLeft);
        root.width = Some(screen.x);
        root.height = Some(screen.y);
        // No TAE node: the timeline is scene-drawn (`draw_tae`), like the graph canvas.
        root.children = vec![self.tool_rail(&r), self.pack_rail(&r)];
        root
    }

    /// The TAE page's boxes: track list | preview | inspector, with the timeline across
    /// the bottom. The design's fixed 260/360/320 are clamped so a smaller window than the
    /// 1920×1080 mock still leaves the preview usable.
    fn tae_regions(screen: Vec2) -> TaeRegions {
        let top = TOP_BAR_H + TAB_BAR_H;
        let track_w = (screen.x * 0.16).clamp(150.0, 260.0);
        let insp_w = (screen.x * 0.22).clamp(200.0, 360.0);
        let strip_h = (screen.y * 0.30).clamp(150.0, 320.0);
        let strip_y = (screen.y - strip_h).max(top + 60.0);
        TaeRegions {
            top,
            track_w,
            insp_w,
            strip_y,
            strip_h,
            preview_w: (screen.x - track_w - insp_w).max(80.0),
            preview_h: (strip_y - top).max(60.0),
            screen,
        }
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

    /// The TAE Editor page: track list | preview + transport | event inspector, over the
    /// scene-drawn timeline. The timeline itself is `draw_tae`, which already owns the
    /// seven-lane geometry — this page only adds the chrome around it.
    fn tae_chrome(&self, screen: Vec2) -> UiNode {
        let r = Self::tae_regions(screen);
        let mut root = node("screen");
        root.id = "tae_page".into();
        root.anchor = Some(UiAnchor::TopLeft);
        root.width = Some(screen.x);
        root.height = Some(screen.y);
        root.children =
            vec![self.tae_track_list(&r), self.tae_preview(&r), self.tae_inspector(&r)];
        root
    }

    /// Left rail: which clip is being edited, and the seven event tracks with live counts.
    fn tae_track_list(&self, r: &TaeRegions) -> UiNode {
        let mut col = node("cell");
        col.id = "tae_tracks".into();
        col.anchor = Some(UiAnchor::TopLeft);
        col.offset = [0.0, r.top];
        col.width = Some(r.track_w);
        col.height = Some(r.strip_y - r.top);
        col.pad = 12.0;
        col.gap = 3.0;
        col = prop(col, "style", text_val("loomforge.rail"));

        let mut kids =
            vec![label_node("EDITING CLIP", 11.0, "loomforge.rail_title.color", Some(18.0))];
        match self.tae_axis() {
            Some((_, clip, frames, rate)) => {
                kids.push(label_node(clip, 16.0, "loomforge.pack_kind.locomotion.color", Some(22.0)));
                kids.push(label_node(
                    format!("{frames} frames · {rate} Hz"),
                    11.0,
                    "loomforge.rail_text.color",
                    Some(16.0),
                ));
            }
            None => kids.push(label_node(
                "Select a state on the graph.",
                12.0,
                "loomforge.rail_text.color",
                Some(30.0),
            )),
        }

        kids.push(label_node("EVENT TRACKS", 11.0, "loomforge.rail_title.color", Some(20.0)));
        let counts = self.tae_lane_counts();
        for (i, lane) in tae::Lane::ALL.iter().enumerate() {
            kids.push(label_node(
                format!("{}   {}", lane.label(), counts[i]),
                13.0,
                &format!("loomforge.tae_lane.{}.swatch", lane.id()),
                Some(17.0),
            ));
        }

        // The two budget gauges. Both numbers are decided HERE and nowhere else, and both
        // would otherwise surface only as bad feel months later — untraceable back to the
        // authoring choice that caused them.
        for (title, line, verdict) in self.tae_budget_rows() {
            kids.push(label_node(title, 11.0, "loomforge.rail_title.color", Some(20.0)));
            kids.push(label_node(
                line,
                12.0,
                &format!("loomforge.tae_lane.{}", verdict.color_key()),
                Some(17.0),
            ));
        }
        col.children = kids;
        col
    }

    /// The gutter's budget readouts for the selected state: `(heading, reading, verdict)`.
    ///
    /// Only shown for the windows that actually set a budget — a state with no parry and no
    /// telegraph has no budget to report, and inventing a row for it would be noise.
    fn tae_budget_rows(&self) -> Vec<(String, String, tae::Budget)> {
        let mut out = Vec::new();
        let Some((_, _, _, rate)) = self.tae_axis() else {
            return out;
        };
        let Some(doc) = &self.doc else { return out };
        let Some(st) = doc.selected().and_then(|i| doc.states().get(i)) else {
            return out;
        };

        // SERVER BUDGET — the earliest parry catch window in this state. `min` because the
        // tightest window is the one the netcode must survive.
        if let Some(tick) = st
            .events
            .iter()
            .filter(|e| e.kind == EventKind::Parry)
            .map(|e| e.tick)
            .min()
        {
            let (ms, verdict) = tae::parry_budget(tick, rate);
            out.push((
                "SERVER BUDGET".into(),
                format!("parry @{tick}  =  {ms:.0} ms commit horizon"),
                verdict,
            ));
        }

        // PLAYER BUDGET — the widest telegraph, against the tier's floor. `max` because the
        // player gets the most generous warning the state offers.
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
                Some(t) => format!("tier {t}"),
                // The creature/encounter model that carries `tier` does not exist yet, so
                // the entry rung is used — say so rather than implying a tier was read.
                None => "no tier yet — entry rung".into(),
            };
            out.push((
                "PLAYER BUDGET".into(),
                format!("telegraph {width}f = {ms:.0} ms  ·  floor {floor:.0} ms ({tier_note})"),
                verdict,
            ));
        }
        out
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

    /// Centre: the 300px Stage doll over the transport bar.
    fn tae_preview(&self, r: &TaeRegions) -> UiNode {
        let mut col = node("cell");
        col.id = "tae_preview".into();
        col.anchor = Some(UiAnchor::TopLeft);
        col.offset = [r.track_w, r.top];
        col.width = Some(r.preview_w);
        col.height = Some(r.preview_h);
        col.pad = 12.0;
        col.gap = 8.0;
        col = prop(col, "style", text_val("loomforge.body"));

        let (frames, rate) = self
            .tae_axis()
            .map(|(_, _, f, rt)| (f, rt))
            .unwrap_or((0, 30));
        let head = self.tae_playhead(frames, rate);

        let mut kids = vec![label_node(
            format!("Frame {head} / {frames}"),
            12.0,
            "loomforge.pack_kind.locomotion.color",
            Some(18.0),
        )];

        // The doll, sized to whatever the preview box allows (the design's 300px cap).
        let side = (r.preview_h - 90.0).min(r.preview_w - 24.0).clamp(64.0, TAE_STAGE_MAX);
        let mut doll = node("rtt");
        doll.id = TAE_STAGE_ID.into();
        doll.width = Some(side);
        doll.height = Some(side);
        doll = prop(doll, "style", text_val("loomforge.clip_stage"));
        doll = prop(doll, "source", text_val(DOLL_SOURCE));
        kids.push(doll);

        // Transport. Step buttons park the playhead; play resumes the shared clock.
        let mut bar = node("row");
        bar.id = "tae_transport".into();
        bar.size = Some(30.0);
        bar.gap = 6.0;
        bar.children = vec![
            self.rail_button("tae_prev", "|<".into(), 30.0),
            self.rail_button("tae_play", if self.tae_playing { "||" } else { ">" }.into(), 30.0),
            self.rail_button("tae_next", ">|".into(), 30.0),
            label_node(
                format!("{:.2}s", self.time % ((frames.max(1) as f32) / rate.max(1) as f32)),
                12.0,
                "loomforge.rail_text.color",
                Some(60.0),
            ),
        ];
        kids.push(bar);
        col.children = kids;
        col
    }

    /// Right rail: the selected event's Timing / Shape / Damage / Feedback groups. These
    /// map 1:1 onto `EventDef` + `CombatMeta`, so every field shown is authored data that
    /// Save writes back — even though no combat runtime consumes it yet.
    fn tae_inspector(&self, r: &TaeRegions) -> UiNode {
        let mut col = node("cell");
        col.id = "tae_inspector".into();
        col.anchor = Some(UiAnchor::TopLeft);
        col.offset = [r.screen.x - r.insp_w, r.top];
        col.width = Some(r.insp_w);
        col.height = Some(r.strip_y - r.top);
        col.pad = 12.0;
        col.gap = 3.0;
        col = prop(col, "style", text_val("loomforge.rail"));

        let Some((ev, lane)) = self.selected_event() else {
            col.children = vec![
                label_node("EVENT", 11.0, "loomforge.rail_title.color", Some(18.0)),
                label_node(
                    "Click an event on the timeline.",
                    12.0,
                    "loomforge.rail_text.color",
                    None,
                ),
            ];
            return col;
        };

        let mut kids = vec![
            label_node(
                format!("{} Event", lane.label()),
                17.0,
                &format!("loomforge.tae_lane.{}.swatch", lane.id()),
                Some(24.0),
            ),
            label_node(
                if ev.label.is_empty() { "—".into() } else { ev.label.clone() },
                11.0,
                "loomforge.rail_text.color",
                Some(16.0),
            ),
            // TIMING
            label_node("TIMING", 11.0, "loomforge.rail_title.color", Some(20.0)),
            self.stepper(format!("Start   {}", ev.tick), "tae_start_dec", "tae_start_inc"),
            self.stepper(
                match ev.end {
                    Some(e) => format!("End   {e}"),
                    None => "End   — (point)".into(),
                },
                "tae_end_dec",
                "tae_end_inc",
            ),
        ];

        let c = ev.combat.as_ref();
        // SHAPE
        kids.push(label_node("SHAPE", 11.0, "loomforge.rail_title.color", Some(20.0)));
        kids.push(label_node(
            format!(
                "Attach Bone   {}",
                c.and_then(|c| c.attach_bone.as_deref()).unwrap_or("—")
            ),
            12.0,
            "loomforge.rail_text.color",
            Some(17.0),
        ));
        kids.push(self.stepper(
            format!(
                "Capsule   {:.2}m",
                c.and_then(|c| c.capsule_radius).unwrap_or(0.35)
            ),
            "tae_cap_dec",
            "tae_cap_inc",
        ));
        // DAMAGE
        kids.push(label_node("DAMAGE", 11.0, "loomforge.rail_title.color", Some(20.0)));
        kids.push(label_node(
            match c.and_then(|c| c.damage) {
                Some([lo, hi]) => format!("Damage   {lo:.0}–{hi:.0}"),
                None => "Damage   —".into(),
            },
            12.0,
            "loomforge.rail_text.color",
            Some(17.0),
        ));
        kids.push(label_node(
            match c.and_then(|c| c.poise_damage) {
                Some(p) => format!("Poise   {p:.0}"),
                None => "Poise   —".into(),
            },
            12.0,
            "loomforge.rail_text.color",
            Some(17.0),
        ));
        // The DC's Hit Type Select — a cycler here, which needs no new walker component
        // and shows the exact value that will be written.
        let mut hit = node("button");
        hit.id = "tae_hit".into();
        hit.action = Some("tae_hit".into());
        hit.height = Some(26.0);
        hit = prop(
            hit,
            "label",
            text_val(format!(
                "Hit Type: {}",
                c.and_then(|c| c.hit_type).map(hit_type_label).unwrap_or("—")
            )),
        );
        kids.push(prop(hit, "style", text_val("loomforge.button_secondary")));
        // RESPONSE — which answers this attack admits. Narrowing to exactly one is what
        // makes an attack perilous, so the heading says which it currently is.
        let mask = c.map(|c| c.response_mask).unwrap_or_default();
        kids.push(label_node(
            if mask.is_perilous() { "RESPONSE — PERILOUS" } else { "RESPONSE" },
            11.0,
            if mask.is_perilous() {
                "loomforge.tae_lane.budget_over"
            } else {
                "loomforge.rail_title.color"
            },
            Some(20.0),
        ));
        for (i, r) in Response::ALL.iter().enumerate() {
            let mut b = node("button");
            b.id = format!("tae_resp_{i}");
            b.action = Some(format!("tae_resp_{i}"));
            b.height = Some(22.0);
            b = prop(b, "label", text_val(response_label(*r)));
            kids.push(prop(b, "style", text_val(active_style(mask.allows(*r)))));
        }
        kids.push(self.stepper(
            format!(
                "Parry scale   {:.2}×",
                c.and_then(|c| c.parry_window_scale).unwrap_or(1.0)
            ),
            "tae_pscale_dec",
            "tae_pscale_inc",
        ));

        // FEEDBACK
        kids.push(label_node("FEEDBACK", 11.0, "loomforge.rail_title.color", Some(20.0)));
        for (k, v) in [
            ("SFX", c.and_then(|c| c.sfx_cue.as_deref()).unwrap_or("—")),
            ("VFX", c.and_then(|c| c.vfx_cue.as_deref()).unwrap_or("—")),
        ] {
            kids.push(label_node(
                format!("{k}   {v}"),
                12.0,
                "loomforge.pack_kind.locomotion.color",
                Some(17.0),
            ));
        }
        col.children = kids;
        col
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

    /// The Pack Browser page: filter rail | pack-card grid | detail pane. Every card is a
    /// real `*.pack.json` from the content tree; the grid never shows a fabricated pack.
    fn pack_chrome(&self, screen: Vec2) -> UiNode {
        let top = TOP_BAR_H + TAB_BAR_H;
        let rail_w = (screen.x * 0.18).clamp(180.0, 280.0);
        let detail_w = (screen.x * 0.24).clamp(240.0, 400.0);
        let grid_w = (screen.x - rail_w - detail_w).max(160.0);
        let h = (screen.y - top).max(80.0);

        let mut root = node("screen");
        root.id = "pack_page".into();
        root.anchor = Some(UiAnchor::TopLeft);
        root.width = Some(screen.x);
        root.height = Some(screen.y);
        root.children = vec![
            self.pack_filter_rail(top, rail_w, h),
            self.pack_grid(rail_w, top, grid_w, h),
            self.pack_detail(screen.x - detail_w, top, detail_w, h),
        ];
        root
    }

    /// Filter rail: pack-kind rows with live counts, then the skeletons actually present.
    /// Both are toggles — an empty selection means "unfiltered", so the browser opens full.
    fn pack_filter_rail(&self, top: f32, w: f32, h: f32) -> UiNode {
        let mut rail = node("cell");
        rail.id = "pack_filter".into();
        rail.anchor = Some(UiAnchor::TopLeft);
        rail.offset = [0.0, top];
        rail.width = Some(w);
        rail.height = Some(h);
        rail.pad = 12.0;
        rail.gap = 4.0;
        rail = prop(rail, "style", text_val("loomforge.rail"));

        let counts = packs::kind_counts(&self.packs);
        let mut kids =
            vec![label_node("FILTER PACKS", 12.0, "loomforge.rail_title.color", Some(20.0))];
        for (i, k) in packs::PackKind::ALL.iter().enumerate() {
            let mut b = node("button");
            b.id = format!("packkind_{i}");
            b.action = Some(format!("packkind_{i}"));
            b.height = Some(24.0);
            b = prop(b, "label", text_val(format!("{}  {}", k.label(), counts[i])));
            kids.push(prop(b, "style", text_val(active_style(self.pack_kinds.contains(k)))));
        }
        kids.push(label_node("SKELETON", 12.0, "loomforge.rail_title.color", Some(22.0)));
        for (i, s) in packs::skeletons(&self.packs).iter().enumerate() {
            let mut b = node("button");
            b.id = format!("packskel_{i}");
            b.action = Some(format!("packskel_{i}"));
            b.height = Some(24.0);
            b = prop(b, "label", text_val(s.clone()));
            kids.push(prop(b, "style", text_val(active_style(self.pack_skels.contains(s)))));
        }
        rail.children = kids;
        rail
    }

    /// The card grid — wrapped into rows because the walker's `row` does not wrap itself.
    fn pack_grid(&self, x: f32, top: f32, w: f32, h: f32) -> UiNode {
        let mut col = node("cell");
        col.id = "pack_grid".into();
        col.anchor = Some(UiAnchor::TopLeft);
        col.offset = [x, top];
        col.width = Some(w);
        col.height = Some(h);
        col.pad = 16.0;
        col.gap = 10.0;
        col = prop(col, "style", text_val("loomforge.body"));

        let vis = self.visible_packs();
        let filtered = !self.pack_kinds.is_empty() || !self.pack_skels.is_empty();
        let count = if filtered {
            format!("{} of {} packs", vis.len(), self.packs.len())
        } else {
            format!("{} pack{}", vis.len(), if vis.len() == 1 { "" } else { "s" })
        };
        let mut kids = vec![
            label_node("Pack Library", 22.0, "loomforge.title.color", Some(28.0)),
            label_node(count, 13.0, "loomforge.rail_text.color", Some(18.0)),
        ];

        // How many cards fit per row at this width, so a narrow window reflows instead of
        // clipping the grid.
        let cols = (((w - 32.0 + PACK_CARD_GAP) / (PACK_CARD_W + PACK_CARD_GAP)).floor() as usize)
            .clamp(1, PACK_GRID_MAX_COLS);
        for (row_i, chunk) in vis.chunks(cols).enumerate() {
            let mut row = node("row");
            row.id = format!("packrow_{row_i}");
            row.gap = PACK_CARD_GAP;
            row.height = Some(PACK_CARD_H);
            row.children = chunk
                .iter()
                .enumerate()
                .map(|(i, e)| self.pack_card(row_i * cols + i, e))
                .collect();
            kids.push(row);
        }
        if vis.is_empty() {
            kids.push(label_node(
                "No packs match this filter.",
                14.0,
                "loomforge.rail_text.color",
                None,
            ));
        }
        col.children = kids;
        col
    }

    /// One pack card: a Stage doll over the pack's name and derived kind.
    fn pack_card(&self, idx: usize, e: &packs::PackEntry) -> UiNode {
        let sel = idx == self.pack_sel;
        let mut card = node("cell");
        card.id = format!("packcard_{idx}");
        card.action = Some(format!("packcard_{idx}"));
        card.width = Some(PACK_CARD_W);
        card.height = Some(PACK_CARD_H);
        card.pad = 8.0;
        card.gap = 3.0;
        card = prop(card, "style", text_val(active_style(sel)));

        let mut thumb = node("rtt");
        thumb.id = format!("{PACK_STAGE_PREFIX}{idx}");
        thumb.width = Some(PACK_STAGE);
        thumb.height = Some(PACK_STAGE);
        thumb = prop(thumb, "style", text_val("loomforge.clip_stage"));
        thumb = prop(thumb, "source", text_val(DOLL_SOURCE));

        card.children = vec![
            thumb,
            label_node(e.name.clone(), 15.0, "loomforge.body_text.color", None),
            label_node(
                format!("{} · {} states", e.kind.label(), e.states),
                11.0,
                e.kind.color_path(),
                None,
            ),
        ];
        card
    }

    /// Detail pane — the selected pack's manifest, read straight off the file.
    fn pack_detail(&self, x: f32, top: f32, w: f32, h: f32) -> UiNode {
        let mut col = node("cell");
        col.id = "pack_detail".into();
        col.anchor = Some(UiAnchor::TopLeft);
        col.offset = [x, top];
        col.width = Some(w);
        col.height = Some(h);
        col.pad = 14.0;
        col.gap = 4.0;
        col = prop(col, "style", text_val("loomforge.rail"));

        let Some(e) = self.selected_pack() else {
            col.children =
                vec![label_node("No packs found.", 14.0, "loomforge.rail_text.color", None)];
            return col;
        };
        let open = self.doc.as_ref().is_some_and(|d| d.path() == e.path);

        let mut kids = vec![
            label_node(e.name.clone(), 20.0, "loomforge.title.color", Some(26.0)),
            label_node(e.kind.label(), 11.0, e.kind.color_path(), Some(16.0)),
            label_node("MANIFEST", 11.0, "loomforge.rail_title.color", Some(20.0)),
        ];
        for (k, v) in [
            ("Format", if e.format.is_empty() { "—".into() } else { e.format.clone() }),
            ("Version", e.version.to_string()),
            ("Skeleton", e.skeleton.clone()),
            ("States", e.states.to_string()),
            ("Transitions", e.transitions.to_string()),
            ("TAE events", format!("{} in {} states", e.events, e.event_states)),
            ("Combat windows", e.combat_events.to_string()),
        ] {
            kids.push(label_node(
                format!("{k}    {v}"),
                13.0,
                "loomforge.rail_text.color",
                Some(17.0),
            ));
        }
        if !e.note.is_empty() {
            kids.push(label_node("NOTE", 11.0, "loomforge.rail_title.color", Some(20.0)));
            for line in wrap_text(&e.note, NOTE_WRAP).into_iter().take(NOTE_MAX_LINES) {
                kids.push(label_node(line, 12.0, "loomforge.rail_text.color", Some(15.0)));
            }
        }

        let mut load = node("button");
        load.id = "pack_load".into();
        load.action = Some("pack_load".into());
        load.height = Some(30.0);
        load = prop(load, "label", text_val(if open { "Loaded" } else { "Load Pack" }));
        kids.push(prop(
            load,
            "style",
            text_val(if open { "loomforge.button_secondary" } else { "loomforge.button_primary" }),
        ));
        col.children = kids;
        col
    }

    fn tool_rail(&self, r: &Regions) -> UiNode {
        let mut rail = node("cell");
        rail.id = "tool_rail".into();
        rail.anchor = Some(UiAnchor::TopLeft);
        rail.offset = [0.0, r.top];
        rail.width = Some(TOOL_RAIL_W);
        rail.height = Some(r.tae_y - r.top);
        rail.pad = 8.0;
        rail.gap = 6.0;
        rail = prop(rail, "style", text_val("loomforge.rail"));

        let kids: Vec<UiNode> = Tool::ALL
            .iter()
            .map(|t| {
                let mut b = node("button");
                b.id = t.id().into();
                b.action = Some(t.id().into());
                b.size = Some(36.0);
                b = prop(b, "label", text_val(t.label()));
                let style = if *t == self.tool {
                    "loomforge.tab_active"
                } else {
                    "loomforge.tab_idle"
                };
                prop(b, "style", text_val(style))
            })
            .collect();

        rail.children = kids;
        rail
    }

    /// Pack Manager: the loaded pack plus its clip library, where every clip row is a
    /// **drag source** (`drag_kind`) the canvas resolves into a `bind_clip`.
    fn pack_rail(&self, r: &Regions) -> UiNode {
        let mut rail = node("cell");
        rail.id = "pack_rail".into();
        rail.anchor = Some(UiAnchor::TopLeft);
        rail.offset = [r.rail_x, r.top];
        rail.width = Some(r.rail_w);
        rail.height = Some(r.tae_y - r.top);
        rail.pad = 14.0;
        rail.gap = 6.0;
        rail = prop(rail, "style", text_val("loomforge.rail"));

        // Selecting an edge swaps this rail over to that transition's inspector: you are
        // editing one edge, not binding clips, and the rail is too narrow for both.
        if let Some(e) = self.selected_edge {
            rail.children = self.transition_rows(e);
            return rail;
        }

        let mut kids = vec![
            label_node("PACK MANAGER", 12.0, "loomforge.rail_title.color", Some(20.0)),
            label_node(self.pack_summary(), 13.0, "loomforge.rail_text.color", Some(18.0)),
            label_node(
                "Drag a clip onto a state to bind it.",
                13.0,
                "loomforge.rail_text.color",
                Some(20.0),
            ),
        ];

        // The trigger the Link tool stamps on new transitions — click to cycle. Lives
        // here rather than the 56px tool rail so the full wire name is legible.
        let mut trig = node("button");
        trig.id = "cycle_trigger".into();
        trig.action = Some("cycle_trigger".into());
        trig.size = Some(28.0);
        trig = prop(
            trig,
            "label",
            text_val(format!("link on: {}", trigger_label(self.link_trigger))),
        );
        trig = prop(trig, "style", text_val("loomforge.button_secondary"));
        kids.push(trig);

        // As many rows as fit, starting from the scroll position — the wheel moves the
        // window over a 91-clip library rather than the rail truncating it.
        let fits = self.clip_page(r);
        let total = self.doc.as_ref().map_or(0, |d| d.clip_names().count());
        let first = self.clip_scroll.min(total.saturating_sub(1));
        if let Some(doc) = &self.doc {
            for name in doc.clip_names().skip(first).take(fits) {
                kids.push(self.clip_row(name));
            }
        }
        let last = (first + fits).min(total);
        if total > fits {
            kids.push(label_node(
                format!("{}–{} of {total} · scroll for more", first + 1, last),
                11.0,
                "loomforge.rail_text.color",
                Some(16.0),
            ));
        }
        rail.children = kids;
        rail
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
            self.status = if ok { "Transition removed.".into() } else { "That transition is gone.".into() };
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
                "{} · priority {} · blend {}",
                trigger_label(t.on),
                t.priority,
                match t.blend_ticks {
                    Some(b) => b.to_string(),
                    None => "default".into(),
                }
            ),
            None => "That transition is gone.".into(),
        };
    }

    /// This frame's transition geometry. One pass, consumed by both picking and drawing.
    fn build_edges(&self) -> Vec<Edge> {
        let Some(doc) = &self.doc else { return Vec::new() };
        let mut out = Vec::new();
        for (i, st) in doc.states().iter().enumerate() {
            let Some(&from) = self.cards.get(i) else { continue };
            for (index, t) in st.transitions.iter().enumerate() {
                let Some(j) = doc.state_index(&t.to) else { continue };
                let Some(&to) = self.cards.get(j) else { continue };
                let id = EdgeRef { from: i, index };
                if i == j {
                    // Self-transition: a loop arc over the card's top edge.
                    let lift = SELF_LOOP_LIFT * self.view.zoom;
                    let (x0, x1) = (from.pos.x + from.size.x * 0.35, from.pos.x + from.size.x * 0.65);
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
                    out.push(Edge { id, segs: [(p, q), zero, zero], n: 1 });
                }
            }
        }
        out
    }

    /// Move the clip-rail window. Positive delta = wheel up = back toward the first clip.
    /// Clamped so the last page is reachable and the list can never scroll past its end.
    fn scroll_clips(&mut self, delta: f32, page: usize) {
        let total = self.doc.as_ref().map_or(0, |d| d.clip_names().count());
        let step = (delta.abs().ceil() as usize).max(1) * CLIP_SCROLL_ROWS;
        self.clip_scroll = if delta > 0.0 {
            self.clip_scroll.saturating_sub(step)
        } else {
            (self.clip_scroll + step).min(total.saturating_sub(page))
        };
    }

    /// How many clip rows fit in the rail — the scroll page size.
    fn clip_page(&self, r: &Regions) -> usize {
        // Header labels + the trigger cycler + the position line, plus the rail padding.
        let used = 60.0 + 28.0 + 16.0 + 14.0 * 2.0;
        ((((r.tae_y - r.top) - used) / (CLIP_ROW_H + 6.0)).floor().max(0.0) as usize).max(1)
    }

    /// The selected transition's inspector: where it goes, what fires it, and the two
    /// dials that decide how it competes and how it blends.
    fn transition_rows(&self, e: EdgeRef) -> Vec<UiNode> {
        let Some(doc) = &self.doc else { return Vec::new() };
        let Some(t) = doc::transition(doc.def(), e) else {
            return vec![label_node(
                "That transition is gone.",
                13.0,
                "loomforge.rail_text.color",
                Some(18.0),
            )];
        };
        let from = doc.states().get(e.from).map_or("?", |s| s.name.as_str());

        let mut rows = vec![
            label_node("TRANSITION", 12.0, "loomforge.rail_title.color", Some(20.0)),
            label_node(format!("{from}  →  {}", t.to), 15.0, "loomforge.title.color", Some(24.0)),
        ];

        let mut trig = self.rail_button("edge_trigger", format!("on: {}", trigger_label(t.on)), 28.0);
        trig = prop(trig, "style", text_val("loomforge.button_secondary"));
        rows.push(trig);

        rows.push(self.stepper(format!("priority  {}", t.priority), "prio_dec", "prio_inc"));
        // `None` inherits the machine default — shown as such rather than as a number,
        // so "inherit" reads as a real state and not as a blend of zero.
        let blend = match t.blend_ticks {
            Some(b) => format!("blend  {b} ticks"),
            None => format!("blend  default ({})", doc.def().default_blend_ticks),
        };
        rows.push(self.stepper(blend, "blend_dec", "blend_inc"));

        if let Some(w) = t.window {
            rows.push(label_node(
                format!("window  {}–{} ticks", w.start, w.end),
                13.0,
                "loomforge.rail_text.color",
                Some(18.0),
            ));
        }

        let mut del = self.rail_button("edge_delete", "Delete Transition".into(), 28.0);
        del = prop(del, "style", text_val("loomforge.button_secondary"));
        rows.push(del);
        rows.push(label_node(
            "Click a state to return to the clip library.",
            12.0,
            "loomforge.rail_text.color",
            Some(18.0),
        ));
        rows
    }

    /// A `label  [-] [+]` stepper — the design's numeric field, built from the walker's
    /// existing button rather than a new component kind.
    fn stepper(&self, label: String, dec: &str, inc: &str) -> UiNode {
        let mut row = node("row");
        row.size = Some(26.0);
        row.gap = 6.0;
        let mut text = node("text");
        text.grow = Some(1.0);
        text = prop(text, "text", text_val(label));
        text = prop(text, "text_size", Value::Number(13.0));
        text = prop(text, "color", text_val("loomforge.rail_text.color"));
        row.children = vec![
            text,
            self.rail_button(dec, "-".into(), 26.0),
            self.rail_button(inc, "+".into(), 26.0),
        ];
        row
    }

    fn rail_button(&self, action: &str, label: String, size: f32) -> UiNode {
        let mut b = node("button");
        b.id = action.into();
        b.action = Some(action.into());
        b.size = Some(size);
        b = prop(b, "label", text_val(label));
        prop(b, "style", text_val("loomforge.button_secondary"))
    }

    /// One clip-library row: a live doll dancing the clip, plus its name.
    ///
    /// The WHOLE ROW is the drag source, so the doll is the handle Aaron reaches for —
    /// "the doll is animating its script and I drag it into the target card". Drag pickup
    /// is prop-driven in the walker, so this needs no bespoke component.
    fn clip_row(&self, name: &str) -> UiNode {
        let mut row = node("row");
        row.id = format!("clip_{name}");
        row.size = Some(CLIP_ROW_H);
        row.pad = 3.0;
        row.gap = 8.0;
        row = prop(row, "style", text_val("loomforge.clip_row"));
        row = prop(row, "drag_kind", text_val("clip"));
        row = prop(row, "drag_id", text_val(name));

        let mut doll = node("rtt");
        doll.id = stage_id(name);
        doll.size = Some(CLIP_STAGE);
        doll = prop(doll, "style", text_val("loomforge.clip_stage"));
        doll = prop(doll, "source", text_val(DOLL_SOURCE));
        // Liveness is authored, not hard-coded: the scene publishes this key from the
        // pointer's position, so only the row under the cursor spends a GPU submit.
        doll = prop(doll, "live_bind", text_val(live_key(name)));

        let mut label = node("text");
        label.grow = Some(1.0);
        label = prop(label, "text", text_val(name));
        label = prop(label, "text_size", Value::Number(13.0));
        label = prop(label, "color", text_val("loomforge.clip_row.label"));

        row.children = vec![doll, label];
        row
    }

    /// One-line description of the loaded pack for the rail header.
    fn pack_summary(&self) -> String {
        match &self.doc {
            Some(d) => format!("{} · {} clips", short_path(d.path()), d.clip_names().count()),
            None => "no pack loaded".into(),
        }
    }

    /// What the TAE strip reports for the current selection.
    fn tae_summary(&self) -> String {
        let Some(doc) = &self.doc else { return "no pack".into() };
        match doc.selected().and_then(|i| doc.states().get(i)) {
            Some(s) => {
                format!("{} · clip \"{}\" · {} event(s)", s.name, s.clip, s.events.len())
            }
            None => "select a state to see its event windows".into(),
        }
    }

    /// The non-graph tabs still report live document facts until their pages land.
    fn summary_body(&self, screen: Vec2) -> UiNode {
        let top = TOP_BAR_H + TAB_BAR_H;
        let mut body = node("cell");
        body.id = "body".into();
        body.anchor = Some(UiAnchor::TopLeft);
        body.offset = [0.0, top];
        body.width = Some(screen.x);
        body.height = Some((screen.y - top).max(0.0));
        body.pad = 24.0;
        body.gap = 10.0;
        body = prop(body, "style", text_val("loomforge.body"));

        let mut lines: Vec<String> = Vec::new();
        if let Some(err) = &self.load_error {
            lines.push(format!("Could not load the pack: {err}"));
        } else if let Some(doc) = &self.doc {
            let d = doc.def();
            match self.tab {
                Tab::StateMachine => {
                    let transitions: usize =
                        doc.states().iter().map(|s| s.transitions.len()).sum::<usize>() + d.any.len();
                    lines.push(format!(
                        "{} states · {} transitions · initial \"{}\"",
                        doc.states().len(),
                        transitions,
                        d.initial
                    ));
                    lines.push(format!("tick rate {} Hz · default blend {} ticks", d.tick_rate_hz, d.default_blend_ticks));
                }
                Tab::PackBrowser => {
                    lines.push(format!("{} clips in the library", doc.clip_names().count()));
                    lines.push(format!("pack: {}", short_path(doc.path())));
                }
                Tab::CreatureComposer => {
                    lines.push("Creature Composer — schema lands with this page.".into());
                }
                Tab::TaeEditor => {
                    let events: usize = doc.states().iter().map(|s| s.events.len()).sum();
                    lines.push(format!("{events} TAE events across {} states", doc.states().len()));
                }
            }
            if !doc.warnings().is_empty() {
                lines.push(format!("{} validation warning(s)", doc.warnings().len()));
            }
        } else {
            lines.push("Loading…".into());
        }
        if !self.status.is_empty() {
            lines.push(self.status.clone());
        }

        body.children = lines
            .iter()
            .map(|l| label_node(l.clone(), 16.0, "loomforge.body_text.color", None))
            .collect();
        body
    }

    fn rig_badge(&self) -> String {
        match &self.doc {
            Some(d) => format!("{} · {} states", short_path(d.path()), d.states().len()),
            None => "no pack".into(),
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

    fn build_stage_reqs(&self, slots: Vec<flicker::ui::RttSlot>) -> Vec<StageReq> {
        let Some(doc) = &self.doc else { return Vec::new() };
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
                let Some(&c) = self.cards.get(i) else { continue };
                let sel = self.selected_is(i);
                let doll = canvas::card_stage_rect(c, self.view.zoom);
                reqs.push(StageReq {
                    id: format!("card_{}#{}", st.name, st.clip),
                    source: DOLL_SOURCE.to_string(),
                    rect: StageRect { pos: doll.pos, size: doll.size },
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
        for (action, delta) in [("tae_cap_dec", -CAPSULE_STEP_M), ("tae_cap_inc", CAPSULE_STEP_M)] {
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
        for (action, delta) in
            [("tae_pscale_dec", -PARRY_SCALE_STEP), ("tae_pscale_inc", PARRY_SCALE_STEP)]
        {
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
        let mut roots: Vec<&Path> = vec![Path::new(BASE_DIR)];
        if let Some(d) = dir.as_deref() {
            roots.push(d);
        }
        match EditorDoc::load(&entry.path, &roots) {
            Ok(doc) => {
                self.status = format!("Loaded {} · {} states", entry.name, doc.states().len());
                self.doc = Some(doc);
                // The graph's layout and selection belong to the pack that is open.
                self.view = canvas::View::default();
                self.selected_edge = None;
                self.link_from = None;
                self.clip_scroll = 0;
            }
            Err(e) => {
                self.status = format!("Could not load {}: {e}", entry.name);
            }
        }
    }

    fn apply_actions(&mut self, results: &ValueMap) {
        for t in Tab::ALL {
            if results.is_on(t.id()) {
                self.tab = t;
            }
        }
        for t in Tool::ALL {
            if results.is_on(t.id()) {
                self.tool = t;
                self.link_from = None; // switching tools abandons a half-drawn edge
            }
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
                    Ok(()) => format!("Saved {}", short_path(doc.path())),
                    Err(e) => format!("Save failed: {e}"),
                },
                None => "Nothing to save.".into(),
            };
        }
        if results.is_on("validate") {
            self.status = match &self.doc {
                Some(doc) if doc.warnings().is_empty() => "Validate: no warnings.".into(),
                Some(doc) => format!("Validate: {}", doc.warnings().join(" · ")),
                None => "Nothing to validate.".into(),
            };
        }
    }
}

impl Scene for LoomforgeBench {
    fn enter(&mut self, renderer: &mut Renderer) {
        self.theme = Some(Theme::build(renderer));
        self.hud_white = Some(renderer.load_texture(&[0xff, 0xff, 0xff, 0xff], 1, 1));
        self.ui_styles = load_styles(HUD_UI_ELEMENTS);

        // The Pack Browser's library: every real `*.pack.json` under the content tree.
        // Scanned once — the browser reads files, it does not watch them.
        self.packs = packs::scan_packs(Path::new(CONTENT_CHARACTERS));
        tracing::info!(packs = self.packs.len(), "loomforge: pack library scanned");

        match EditorDoc::load(Path::new(PACK_PATH), &[Path::new(BASE_DIR), Path::new(CLIPS_DIR)]) {
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
    }

    fn update(&mut self, dt: Duration, input: &InputState, renderer: &Renderer) -> Transition {
        // Input arbitration (Esc → pause + the HUD pointer-consume) now runs through the
        // ONE event bus in the resolve → dispatch block below, once this frame's walker
        // pass has produced `over_hud`. The node-graph / TAE-timeline tools keep their
        // own bespoke raw-pointer picking/drag and their `over_hud` gate.
        let screen = renderer.size();
        self.cursor = input.mouse_position;
        // One clock for every doll — each clip loops on its own duration.
        self.time += dt.as_secs_f32();
        // Canvas geometry FIRST, so this frame's hit-testing (select / drop) and the
        // draw pass agree on one layout.
        let regions = Self::regions(screen);
        self.canvas_area = regions.canvas;
        self.cards = match &self.doc {
            Some(d) => canvas::layout(
                d.states().iter().map(|s| s.name.as_str()),
                regions.canvas,
                &self.view,
            ),
            None => Vec::new(),
        };
        self.edges = self.build_edges();

        let tree = self.build_tree(screen);
        // The screen's declarative bindings (S9): the root props are static even
        // though the tree rebuilds per frame, so collect them once from the first
        // build (empty only until then — loomforge's root always declares).
        if self.ui_intents.is_empty() {
            self.ui_intents = UiIntents::of(&tree);
        }
        // The one doll under the pointer animates; the rest hold their poster. Resolved
        // from last frame's rects, so hover costs no extra layout pass — and a frame of
        // lag on "start dancing" is imperceptible.
        let mut model = ValueMap::new();
        if let Some(clip) = self.hot_stage.as_deref().and_then(|h| h.strip_prefix(STAGE_PREFIX)) {
            model.set(live_key(clip), true);
        }
        // The transient `sig_<name>` mirror (S9): intent names fired last frame
        // ride exactly this ONE publish, then drop (cleared after the walk).
        UiIntents::mirror_into(&mut model, &self.fired_sigs);
        let snap = UiInput {
            mouse: input.mouse_position,
            clicked: input.mouse_left_pressed,
            down: input.mouse_left,
            screen,
            typed: String::new(),
            backspace: false,
            wheel: input.mouse_wheel_delta,
        };
        let lib = self.ui_lib.as_ref().map(|h| h as &dyn ComponentLibrary);
        let frame = run_ui_with(&tree, &model, &self.ui_styles, &snap, &mut self.ui_state, lib);
        // Copy out what the canvas needs before `frame` is consumed / `self` mutated.
        let over_hud = frame.results.is_on("hud_hit");
        let dropped = frame.results.is_on("drag_dropped");
        let drag_is_clip = frame.results.text("drag_kind") == Some("clip");
        let drag_id = frame.results.text("drag_id").map(str::to_string);
        self.hud_commands = frame.commands;
        let slots = frame.rtts;
        self.apply_actions(&frame.results);

        // Dolls for this frame, and which one the pointer is inside (for the next).
        self.stage_reqs = self.build_stage_reqs(slots);
        self.hot_stage = self
            .stage_reqs
            .iter()
            .find(|r| r.id.starts_with(STAGE_PREFIX) && rect_contains(&r.rect, self.cursor))
            .map(|r| r.id.clone());

        // ── The input seam (spec §5/§9): ONE resolve + ONE dispatch replaces the old
        // `menu_prev` edge. The resolver turns this frame's raw snapshot into discrete
        // `Fired` edges over the active map; each is wrapped with the active context and
        // routed through the chain (root → walker). `ev` is the REUSED scratch buffer;
        // the `InputEvent` list is a short-lived local because it borrows THIS frame's
        // snapshot (RT-7: steady-state frames resolve zero edges and allocate nothing). ──
        self.tick = self.tick.wrapping_add(1);
        self.ev.clear();
        self.resolver.resolve_frame(
            &self.bindings,
            &self.gamepad_config,
            input,
            self.tick,
            &mut self.ev,
        );
        let ctx = self.bindings.active();
        let events: Vec<InputEvent> = self
            .ev
            .iter()
            .map(|f| InputEvent::from_fired(f, ctx, input))
            .collect();

        // Chain: the scene root (declares World) then the walker layer, which consumes
        // the pointer signal while `over_hud` (the canonical `hud_hit`) and the screen's
        // DECLARED intents (S9: the root's `on_menu = "pause_open"`). The bespoke tools
        // below still read the raw pointer and gate on `over_hud`, so this walker layer
        // only formalizes the arbitration for anything on the bus.
        self.fired_sigs.clear(); // last frame's mirror rode the walk above — done
        let mut root = RootHandler;
        let mut walker =
            WalkerHandler::hud(&mut self.ui_state, over_hud).with_intents(&self.ui_intents);
        {
            let mut chain: [&mut dyn InputHandler; 2] = [&mut root, &mut walker];
            Router::dispatch(&events, &mut chain, &mut self.route);
        }
        // Reconcile any context push/pop the chain queued (none in this editor today) and
        // clear the queue for next frame — keeps the seam complete if a modal is added.
        apply_context_requests(&mut self.bindings, &self.route.requests);
        // The screen's fired intents (S9), drained once: acted on below and queued for
        // the one-frame `sig_<name>` Model mirror.
        self.fired_sigs = walker.take_fired();
        self.route.requests.clear();

        // The screen DECLARED `on_menu = "pause_open"` (S9): the walker layer consumed
        // the Menu press and fired the name; the scene maps it onto its pause push —
        // the root's hardcoded Menu arm is gone.
        if self.fired_sigs.iter().any(|n| n == "pause_open") {
            if let Some(theme) = self.theme {
                return Transition::Push(Box::new(PauseScene::new(
                    theme,
                    self.bindings.active_map(),
                    &AbstractControls::default(),
                    &self.gamepad_config,
                )));
            }
        }

        if self.tab == Tab::StateMachine {
            let hit = canvas::hit_test(&self.cards, input.mouse_position);
            let released = self.prev_down && !input.mouse_left;

            // ── Navigating the workspace ──────────────────────────────────────────
            // The wheel means "more of what is under the cursor": over the clip library
            // it pages the list, over the graph it zooms about the pointer.
            if input.mouse_wheel_delta != 0.0 {
                if self.cursor.x >= regions.rail_x
                    && self.cursor.y >= regions.top
                    && self.cursor.y < regions.tae_y
                {
                    let page = self.clip_page(&regions);
                    self.scroll_clips(input.mouse_wheel_delta, page);
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
            // card behind it, AND on the canvas rect — the timeline is scene-drawn, so
            // chrome coverage alone would let a click there place or delete a state.
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
                                self.status = match self.doc.as_ref().and_then(|d| doc::transition(d.def(), e)) {
                                    Some(t) => format!("Editing the {} edge.", trigger_label(t.on)),
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
                                    Some(i) => format!("Added state {} (clip \"{c}\")", i + 1),
                                    None => "Could not add a state.".into(),
                                },
                                None => "No clips loaded — cannot add a state.".into(),
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
                                self.status = format!("Removed {name} (and its edges)");
                                self.selected_edge = None;
                            }
                        }
                        None => {
                            if let Some(e) = self.hit_edge(self.cursor) {
                                if self.doc.as_mut().is_some_and(|d| d.remove_transition(e)) {
                                    self.status = "Transition removed.".into();
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
                        "A state cannot transition to itself here.".into()
                    } else if self.doc.as_mut().is_some_and(|d| d.add_transition(from, to, on)) {
                        format!("Linked on {}", trigger_label(on))
                    } else {
                        format!("That {} edge already exists.", trigger_label(on))
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
                                format!("Bound \"{id}\" to {name}")
                            } else {
                                format!("Could not bind \"{id}\" — unknown clip or state")
                            }
                        }
                        None => format!("\"{id}\" dropped outside a state"),
                    };
                }
            }
        }

        // TAE Editor: a press on the scene-drawn timeline (never over the chrome) selects
        // the event under the cursor, opening it in the inspector. A press on empty track
        // clears the selection, matching the graph canvas's "click nothing = deselect".
        if self.tab == Tab::TaeEditor && input.mouse_left_pressed && !over_hud {
            let strip = self.tae_page_strip(screen);
            if rect_contains(
                &StageRect { pos: strip.pos, size: strip.size },
                input.mouse_position,
            ) {
                self.tae_event = self.tae_pick_event(input.mouse_position, screen);
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
                self.stage_rig.stage(renderer, &mut fg, doc.model(), base, &self.stage_reqs);
                fg.execute(renderer);
            }
        }
        // Amortised cleanup: a page change strands the previous page's targets, but a
        // steady page never trips this, so the common frame does no bookkeeping at all.
        if self.stage_rig.slot_count() > self.stage_reqs.len() + STALE_SLOT_SLACK {
            let shown: Vec<&str> = self.stage_reqs.iter().map(|r| r.id.as_str()).collect();
            self.stage_rig.retain_slots(renderer, &|id| shown.contains(&id));
        }

        // The scene-owned canvas paints on the base layer, UNDER the walker chrome —
        // otherwise the rails' panels and the cards would interleave by submission
        // order within one layer (the paperdoll HUD-behind-panels trap). The dolls
        // composited above ride the sprite pass, which runs after ui-panels within a
        // layer, so each lands over its own backdrop without a layer of its own.
        if self.tab == Tab::StateMachine {
            self.draw_canvas(renderer);
            self.draw_tae(renderer, &Self::regions(renderer.size()));
        }
        if self.tab == Tab::TaeEditor {
            self.draw_tae_page(renderer);
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
            let w = if picked { 3.0 } else if lit { 2.4 } else { 1.4 };
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
            let Some(&c) = self.cards.get(i) else { continue };
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
            r.draw_ui_panel(doll.pos, doll.size, stage_top, stage_bot, 1.0, 4.0 * z, 1.0, stage_border, 0.0);
            r.draw_text(
                &st.name,
                c.pos + Vec2::new(text_x, 12.0 * z),
                17.0 * z,
                if sel { label_sel } else { label },
            );
            let io = format!("IN {} · OUT {}", in_deg[i], st.transitions.len());
            r.draw_text(&io, c.pos + Vec2::new(text_x, 38.0 * z), 11.0 * z, meta);
            r.draw_text(&st.clip, c.pos + Vec2::new(text_x, 56.0 * z), 12.0 * z, meta);
        }

        // Rubber-band for an in-flight Link drag, drawn last so it rides over the cards.
        if let Some(from) = self.link_from.and_then(|i| self.cards.get(i)) {
            let hint = self.color("loomforge.canvas.drop_hint", [0.435, 0.592, 1.0, 1.0]);
            draw_line(r, from.center(), self.cursor, 2.0, hint);
        }
    }

    /// Draw the TAE timeline: header, frame ruler, the seven lanes, the selected state's
    /// events, and the playhead.
    ///
    /// The playhead runs off the SAME clock as the Stage dolls, so the bar sweeping the
    /// timeline and the pose on the selected card are showing the same instant.
    fn draw_tae(&self, r: &mut Renderer, region: &Regions) {
        let width = region.screen.x;
        let top = self.color("loomforge.tae.fill_top", [0.055, 0.063, 0.086, 1.0]);
        let bot = self.color("loomforge.tae.fill_bot", [0.031, 0.035, 0.047, 1.0]);
        let frame_c = self.color("loomforge.tae.border", [0.431, 0.353, 0.204, 0.35]);
        r.draw_ui_panel(
            Vec2::new(0.0, region.tae_y),
            Vec2::new(width, region.tae_h),
            top,
            bot,
            1.0,
            0.0,
            1.0,
            frame_c,
            0.0,
        );

        let title_c = self.color("loomforge.rail_title.color", [0.722, 0.592, 0.353, 1.0]);
        let text_c = self.color("loomforge.rail_text.color", [0.871, 0.847, 0.788, 1.0]);
        r.draw_text("TAE TIMELINE", Vec2::new(12.0, region.tae_y + 4.0), 12.0, title_c);
        r.draw_text(&self.tae_summary(), Vec2::new(tae::GUTTER_W + 12.0, region.tae_y + 4.0), 12.0, text_c);

        let strip = tae::Strip {
            pos: Vec2::new(0.0, region.tae_y + TAE_HEADER_H),
            size: Vec2::new(width, (region.tae_h - TAE_HEADER_H).max(1.0)),
        };
        self.draw_tae_strip(r, strip);
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
        let track_border = self.color("loomforge.tae_lane.track_border", [0.149, 0.169, 0.208, 1.0]);
        for f in tae::ruler_ticks(frames) {
            let x = strip.frame_x(f, frames);
            r.draw_text(&f.to_string(), Vec2::new(x + 3.0, strip.pos.y), 10.0, ruler_c);
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
            r.draw_ui_panel(rect.pos, rect.size, row, row, 0.0, 3.0, 1.0, row_border, 0.0);
            // The design's 9px lane chip, then the label beside it.
            let chip = (rect.size.y * 0.4).clamp(4.0, 9.0);
            let cy = rect.pos.y + (rect.size.y - chip) * 0.5;
            r.draw_ui_panel(Vec2::new(8.0, cy), Vec2::splat(chip), swatch, swatch, 0.0, 1.0, 0.0, [0.0; 4], 0.0);
            r.draw_text(lane.label(), Vec2::new(8.0 + chip + 6.0, cy - 1.0), 10.0, swatch);
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
                == sel_state.map(|s| doc::EventRef { state: s, index: idx });
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

    /// The TAE Editor page's full-height timeline: the same lane strip as the preview, but
    /// filling the page's bottom band and pickable (click an event bar to inspect it).
    fn draw_tae_page(&self, r: &mut Renderer) {
        let reg = Self::tae_regions(r.size());
        let top = self.color("loomforge.tae.fill_top", [0.055, 0.063, 0.086, 1.0]);
        let bot = self.color("loomforge.tae.fill_bot", [0.031, 0.035, 0.047, 1.0]);
        let frame_c = self.color("loomforge.tae.border", [0.431, 0.353, 0.204, 0.35]);
        r.draw_ui_panel(
            Vec2::new(reg.track_w, reg.strip_y),
            Vec2::new(reg.screen.x - reg.track_w, reg.strip_h),
            top,
            bot,
            1.0,
            0.0,
            1.0,
            frame_c,
            0.0,
        );
        let title_c = self.color("loomforge.rail_title.color", [0.722, 0.592, 0.353, 1.0]);
        r.draw_text("TIMELINE", Vec2::new(reg.track_w + 12.0, reg.strip_y + 4.0), 12.0, title_c);
        self.draw_tae_strip(
            r,
            tae::Strip {
                pos: Vec2::new(reg.track_w, reg.strip_y + TAE_HEADER_H),
                size: Vec2::new(
                    reg.screen.x - reg.track_w,
                    (reg.strip_h - TAE_HEADER_H).max(1.0),
                ),
            },
        );
    }

    /// The full-page timeline's strip rect, so click-picking and drawing agree.
    fn tae_page_strip(&self, screen: Vec2) -> tae::Strip {
        let reg = Self::tae_regions(screen);
        tae::Strip {
            pos: Vec2::new(reg.track_w, reg.strip_y + TAE_HEADER_H),
            size: Vec2::new(reg.screen.x - reg.track_w, (reg.strip_h - TAE_HEADER_H).max(1.0)),
        }
    }

    /// Pick the event under the cursor on the full-page timeline. Returns the event to
    /// select — a bar hit within its lane, nearest-first so overlapping windows resolve to
    /// the closest edge. Point events get the grab radius so a 5px marker is still clickable.
    fn tae_pick_event(&self, cursor: Vec2, screen: Vec2) -> Option<doc::EventRef> {
        let doc = self.doc.as_ref()?;
        let state = doc.selected()?;
        let st = doc.states().get(state)?;
        let (frames, _) = doc.clip_axis(&st.clip)?;
        let strip = self.tae_page_strip(screen);
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

    /// Read an rgba from the resolved `ui_elements.json` by dotted path. The canvas is
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

/// Launcher factory — the `prism-alpha` roster entry constructs the scene through this.
pub fn scene() -> Box<dyn Scene> {
    Box::new(LoomforgeBench::new())
}

// ── small UiNode builders ────────────────────────────────────────────────────

fn node(component: &str) -> UiNode {
    UiNode { component: component.to_string(), ..Default::default() }
}

fn prop(mut n: UiNode, key: &str, value: Value) -> UiNode {
    n.props.insert(key.to_string(), value);
    n
}

fn text_val(s: impl Into<String>) -> Value {
    Value::Text(s.into())
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

fn rect_contains(r: &StageRect, p: Vec2) -> bool {
    p.x >= r.pos.x && p.x <= r.pos.x + r.size.x && p.y >= r.pos.y && p.y <= r.pos.y + r.size.y
}

/// A `text` node: `color` is a dotted path into the resolved `ui_elements.json`.
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

/// The lit/idle style pair every toggle in the bench uses, so a selected filter row, an
/// active tab, and a selected card can never drift apart visually.
fn active_style(on: bool) -> &'static str {
    if on {
        "loomforge.tab_active"
    } else {
        "loomforge.tab_idle"
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

fn label_node(text: impl Into<String>, size: f32, color_path: &str, width: Option<f32>) -> UiNode {
    let mut n = node("text");
    n.size = width;
    n = prop(n, "text", text_val(text));
    n = prop(n, "text_size", Value::Number(size as f64));
    prop(n, "color", text_val(color_path))
}

/// Last two path components — enough to identify a pack without the full path.
fn short_path(p: &Path) -> String {
    let file = p.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    match p.parent().and_then(|d| d.file_name()) {
        Some(dir) => format!("{}/{}", dir.to_string_lossy(), file),
        None => file,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The tab set + their ids/labels are what the chrome and the action router agree
    /// on — a mismatch would silently break tab switching.
    #[test]
    fn tabs_have_unique_ids_and_labels() {
        let mut ids: Vec<&str> = Tab::ALL.iter().map(|t| t.id()).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), Tab::ALL.len(), "tab ids must be unique");
        assert!(Tab::ALL.iter().all(|t| !t.label().is_empty()));
        assert_eq!(Tab::default(), Tab::StateMachine, "the bench opens on the graph");
    }

    /// The chrome builds without a document (the pre-load / load-failure frame) and
    /// still produces the top bar + one button per tab, so the UI can never be blank.
    #[test]
    fn chrome_builds_without_a_document() {
        let bench = LoomforgeBench::new();
        let tree = bench.build_tree(Vec2::new(1920.0, 1080.0));
        assert_eq!(tree.children.len(), 3, "top bar + tab bar + body");
        let tab_bar = &tree.children[1];
        assert_eq!(tab_bar.children.len(), Tab::ALL.len());
        for (b, t) in tab_bar.children.iter().zip(Tab::ALL) {
            assert_eq!(b.action.as_deref(), Some(t.id()));
        }
    }

    /// Clicking a tab button routes to that page.
    #[test]
    fn tab_actions_switch_the_page() {
        let mut bench = LoomforgeBench::new();
        let mut results = ValueMap::new();
        results.set(Tab::TaeEditor.id(), true);
        bench.apply_actions(&results);
        assert_eq!(bench.tab(), Tab::TaeEditor);
    }

    /// Tool selection + trigger cycling route through the same action map as the tabs.
    #[test]
    fn tool_and_trigger_actions_route() {
        let mut bench = LoomforgeBench::new();
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
        assert!(bench.link_from.is_none(), "switching tools abandons the pending edge");
    }

    /// The Phase-3 deliverable end to end, against the REAL pack: load → bind a clip
    /// (the drag-drop edit) → save → reload, with the `_note` header surviving.
    #[test]
    fn load_bind_save_round_trips_to_disk() {
        let pack_path = Path::new(PACK_PATH);
        if !pack_path.exists() {
            return; // content not present in this checkout — nothing to assert
        }
        let mut doc = EditorDoc::load(pack_path, &[Path::new(BASE_DIR), Path::new(CLIPS_DIR)])
            .expect("the shipped pack + clip library must load");
        assert!(!doc.states().is_empty(), "pack has states");

        // A bad drop is rejected and must NOT dirty the document.
        assert!(!doc.bind_clip(0, "definitely_not_a_real_clip"));
        assert!(!doc.dirty(), "a rejected drop must not dirty the document");
        assert!(!doc.bind_clip(usize::MAX, "whatever"), "unknown state rejected too");

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
        assert_eq!(reloaded.state_machine.states[0].clip, clip, "the edit persisted");
        assert_eq!(reloaded.note, note, "the hand-authored _note survived the save");
        assert_eq!(
            reloaded.state_machine.states.len(),
            doc.states().len(),
            "no states lost in the round trip"
        );
        let _ = std::fs::remove_file(&tmp);
    }

    /// A clip row is the drag handle AND carries the doll. The `stage` node must name a
    /// source that actually exists in the shared `stages` block, and bind its liveness to
    /// the key the scene publishes — a typo in either silently costs a GPU submit per
    /// row, or a doll that never animates.
    #[test]
    fn clip_row_carries_a_bound_doll_and_is_a_drag_source() {
        let bench = LoomforgeBench::new();
        let row = bench.clip_row("walk_forward");

        assert_eq!(row.props.get("drag_kind"), Some(&text_val("clip")));
        assert_eq!(row.props.get("drag_id"), Some(&text_val("walk_forward")));

        let doll = row
            .children
            .iter()
            .find(|c| c.component == "rtt")
            .expect("the row carries a doll");
        assert_eq!(doll.props.get("source"), Some(&text_val(DOLL_SOURCE)));
        assert_eq!(
            doll.props.get("live_bind"),
            Some(&text_val(live_key("walk_forward"))),
            "liveness must bind to the key the scene publishes"
        );
        assert!(doll.props.contains_key("style"), "the doll has a backdrop frame");

        // The id is the stage cache key; the scene reads the clip name back off it.
        assert_eq!(doll.id, stage_id("walk_forward"));
        assert_eq!(doll.id.strip_prefix(STAGE_PREFIX), Some("walk_forward"));

        // And the source it names must be one the shared JSON actually defines.
        let styles = load_styles(HUD_UI_ELEMENTS);
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

    /// Every pack-kind accent must resolve in the shared JSON. These are `text` node
    /// colour paths, which fall back silently, so a rename would show up only as grey
    /// labels in the window — pin them here instead.
    #[test]
    fn every_pack_kind_colour_resolves_in_the_shared_json() {
        let mut bench = LoomforgeBench::new();
        bench.ui_styles = load_styles(HUD_UI_ELEMENTS);
        let miss = [-1.0, -1.0, -1.0, -1.0];
        for k in packs::PackKind::ALL {
            let c = bench.color(k.color_path(), miss);
            assert_ne!(c, miss, "{} is missing from ui_elements.json", k.color_path());
            assert!(c.iter().all(|v| v.is_finite()));
        }
    }

    /// The Pack Browser page must build a well-formed tree over the REAL library: unique
    /// node ids (duplicate ids would make two cards report the same action) and a card
    /// per visible pack.
    #[test]
    fn pack_browser_tree_is_well_formed_over_the_real_library() {
        let mut bench = LoomforgeBench::new();
        bench.ui_styles = load_styles(HUD_UI_ELEMENTS);
        bench.packs = packs::scan_packs(Path::new(CONTENT_CHARACTERS));
        if bench.packs.is_empty() {
            return; // content tree absent in this checkout
        }
        bench.tab = Tab::PackBrowser;
        let tree = bench.body(Vec2::new(1920.0, 1080.0));

        let mut ids: Vec<String> = Vec::new();
        collect_ids(&tree, &mut ids);
        let mut sorted = ids.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), ids.len(), "duplicate node id in the pack page");

        // One card per visible pack, and a filter row per kind.
        let vis = bench.visible_packs().len();
        assert_eq!(
            ids.iter().filter(|i| i.starts_with("packcard_")).count(),
            vis,
            "one card per visible pack"
        );
        assert_eq!(
            ids.iter().filter(|i| i.starts_with("packkind_")).count(),
            packs::PackKind::ALL.len()
        );
        // Every card carries a Stage doll the scene can resolve back to its pack.
        assert_eq!(ids.iter().filter(|i| i.starts_with(PACK_STAGE_PREFIX)).count(), vis);
    }

    /// The TAE Editor page must build a well-formed tree over the real Katanami pack:
    /// unique ids, seven lane rows in the track list, and the transport buttons present.
    #[test]
    fn tae_page_tree_is_well_formed() {
        let doc =
            EditorDoc::load(Path::new(PACK_PATH), &[Path::new(BASE_DIR), Path::new(CLIPS_DIR)]);
        let Ok(doc) = doc else { return }; // content tree absent
        let mut bench = LoomforgeBench::new();
        bench.ui_styles = load_styles(HUD_UI_ELEMENTS);
        bench.doc = Some(doc);
        bench.tab = Tab::TaeEditor;
        // Select a state so the axis + track counts resolve.
        if let Some(d) = bench.doc.as_mut() {
            d.select(Some(0));
        }
        let tree = bench.body(Vec2::new(1920.0, 1080.0));

        let mut ids: Vec<String> = Vec::new();
        collect_ids(&tree, &mut ids);
        let mut uniq = ids.clone();
        uniq.sort();
        uniq.dedup();
        assert_eq!(uniq.len(), ids.len(), "duplicate node id in the TAE page");
        assert!(ids.iter().any(|i| i == TAE_STAGE_ID), "the preview doll must be present");
        for b in ["tae_play", "tae_prev", "tae_next"] {
            assert!(ids.iter().any(|i| i == b), "{b} transport button missing");
        }
    }

    /// Every lane-swatch style the track list and inspector read by dotted path must
    /// resolve — a rename would show up only as grey lane labels in the window.
    #[test]
    fn every_tae_lane_swatch_resolves() {
        let mut bench = LoomforgeBench::new();
        bench.ui_styles = load_styles(HUD_UI_ELEMENTS);
        let miss = [-1.0, -1.0, -1.0, -1.0];
        for lane in tae::Lane::ALL {
            let path = format!("loomforge.tae_lane.{}.swatch", lane.id());
            assert_ne!(bench.color(&path, miss), miss, "{path} missing");
        }
    }

    /// Filtering must never leave the detail pane describing a hidden pack.
    #[test]
    fn filtering_reclamps_the_selection() {
        let mut bench = LoomforgeBench::new();
        bench.packs = packs::scan_packs(Path::new(CONTENT_CHARACTERS));
        if bench.packs.len() < 2 {
            return;
        }
        bench.pack_sel = bench.visible_packs().len() - 1;
        // Filter down to the first pack's kind only.
        let kind = bench.packs[0].kind;
        let mut results = ValueMap::default();
        let idx = packs::PackKind::ALL.iter().position(|k| *k == kind).unwrap();
        results.set(&format!("packkind_{idx}"), true);
        bench.apply_pack_actions(&results);

        let vis = bench.visible_packs();
        assert!(!vis.is_empty());
        assert!(bench.pack_sel < vis.len(), "selection must address a visible card");
        assert!(vis.iter().all(|e| e.kind == kind));
        // The selected entry is one of the visible ones.
        assert!(bench.selected_pack().is_some());
    }

    /// The doll's backdrop styles must resolve to real rgba. `color()` falls back
    /// silently on a missing path, so a renamed key would go unnoticed until someone
    /// looked at the window — exactly the brittleness of scene-drawn dotted lookups.
    #[test]
    fn card_stage_colours_resolve_in_the_shared_json() {
        let mut bench = LoomforgeBench::new();
        bench.ui_styles = load_styles(HUD_UI_ELEMENTS);
        let miss = [-1.0, -1.0, -1.0, -1.0];
        for path in [
            "loomforge.canvas.stage_top",
            "loomforge.canvas.stage_bot",
            "loomforge.canvas.stage_border",
            "loomforge.canvas.card_label",
            "loomforge.canvas.edge",
        ] {
            let c = bench.color(path, miss);
            assert_ne!(c, miss, "{path} is missing from ui_elements.json");
            assert!(c.iter().all(|v| v.is_finite()), "{path} resolved to a non-colour");
        }
        // The clip-row doll's frame is a walker `style`, so it must be a real block.
        assert!(
            bench.ui_styles.pointer("/loomforge/clip_stage").is_some(),
            "loomforge.clip_stage must exist for the clip-row doll"
        );
    }

    /// Every lane's four colours must resolve in the shared JSON. `lane_color` falls back
    /// silently, so a renamed key would leave a lane drawn in placeholder grey with
    /// nothing failing — the exact failure mode of scene-drawn dotted-path lookups.
    #[test]
    fn every_tae_lane_resolves_all_four_colours() {
        let mut bench = LoomforgeBench::new();
        bench.ui_styles = load_styles(HUD_UI_ELEMENTS);
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

    /// Save/validate with no document must report, not panic.
    #[test]
    fn actions_are_safe_without_a_document() {
        let mut bench = LoomforgeBench::new();
        let mut results = ValueMap::new();
        results.set("save", true);
        bench.apply_actions(&results);
        assert!(bench.status.contains("Nothing to save"));
    }
}
