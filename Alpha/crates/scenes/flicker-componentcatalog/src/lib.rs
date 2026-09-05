//! flicker-componentcatalog: the UI TEST scene — and, since the requirements review
//! (B05B3D09), the REGISTRY OF RECORD for everything the engine draws (A466E4C7):
//! anything drawn as a component and missing from here is a duplicate to fold or a
//! component to migrate.
//!
//! Four sections flow top-to-bottom in one scrollable tray on the right, with a left
//! nav rail of bookmarks that SCROLLS the tray to a card (the bookmark of the card at
//! the top of the view highlights):
//!
//! 1. **KINDS** — one demo copy of every Rust widget component, all features on. The
//!    demand is DERIVED from the public roster ([`RUST_COMPONENT_KINDS`]), never
//!    counted by hand (D4, DCA4DFB2); a newly promoted kind fails the build by name
//!    until its card is authored.
//! 2. **RECIPES** — the canonical ARRANGEMENTS (toast + Undo, property row, tree row,
//!    breadcrumb, notice card, transport bar, resource readout, command card, legend
//!    row, collapsible group, drag → drop). A recipe is the standing answer to "that
//!    looks like a missing component": decompose before promoting (F1BFA408).
//! 3. **FILLERS** — the engine-crate surface fillers, each seated LIVE on a plain
//!    `surface`: Plot, GraphCanvas, Timeline, Gadget, Doll. Their demand is a PINNED
//!    roster in the gates (no engine list names them).
//! 4. **MODALS** — one button per shared tree in `scenes/shared/`, opened by id
//!    through `SharedModal::open`, with the `modal_closed` answer read back. The
//!    catalog is the exerciser of that seam.
//!
//! A Developer-realm bench (like Click Trainer), on the ratified pattern:
//! - **template-free** — `componentcatalog.scene.json` names primitive component KINDS
//!   directly (201F4F51), loaded by the bench; every display string is a `$token`.
//! - **pair-script Lua** (five-line split, 491BD9BB) — `componentcatalog.lua` owns the
//!   scene's component logic (demo seeds, nav highlight, the Paged Menu gates); the
//!   tray itself is static and the scroll-to stays pure engine wiring.
//! - **on the pump** (input-P3, 0569DA9B): owns no resolver — the PUMP resolves this
//!   frame's events and hands them in via [`SceneInput`].
//!
//! Scroll-to: the walker reports every card box's resolved rect ([`UiFrame::rect`]); the
//! scroll offset that brings card `i` to the top is `card_i.y - card_0.y` (the height
//! stacked above it), read live from the layout — no hardcoded heights. Esc opens pause.

use std::time::Duration;

use flicker::render::{
    grid_segments_xy, FrameGraph, Rect, Renderer, TextureHandle, Vec2, Vec3, ViewportFiller,
    ViewportLayout,
};
use flicker::scene::{Scene, SceneInput, Transition};
use flicker::script::{HudCommand, ScriptHost, UiNode, ValueMap};
use flicker::ui::{
    instantiate_rows, render_hud, run_ui, strings, Plot, PlotKind, PlotSeries, PlotStyle, Row,
    SceneDef, SurfacePointer, SurfaceSlot, UiInput, UiIntents, UiState, WalkerHandler,
};
use flicker_canvas::{
    CanvasMetrics, CanvasStyle, EdgeInk, GraphCanvas, GraphEdge, GraphNode, LaneStyle,
    PointerSample, Timeline, TimelineEvent, TimelineLane, TimelineMetrics, TimelineStyle,
};
use flicker_input_core::{AbstractControls, GamepadConfig, InputContext, InputMap, InputState};
use flicker_input_router::{InputHandler, Router};
use flicker_rigview::{gadget::modes_from_names, Doll, Gadget, GadgetStyle, Projection, RigView};
use flicker_shell::{
    ModalConflict, ModalOption, ModalParams, ModalProgress, ModalText, PauseScene, SharedModal,
    Theme,
};
use glam::Mat3;

/// The scene's PAIR SCRIPT (`SceneName.lua` — the scene's component logic:
/// demo seeds, nav highlight, the Paged Menu card's page/tab gates).
const CATALOG_SCRIPT: &str =
    include_str!("../../../../content/sensorium/scripts/componentcatalog.lua");

/// The scrollable content tray's `bind` — the offset (px) the bench sets to scroll-to a
/// card, and the wheel writes as you scroll.
const CONTENT_SCROLL_BIND: &str = "cat_content_scroll";

/// Every `card_<i>` id in the authored tray, in tree order — DERIVED from the tree
/// at load, never counted by hand. The card list, the nav loop, and the section
/// tracker all walk this, so adding a card is one authored box + its bookmark and
/// zero bookkeeping (the roster-coverage gate below tells you when one is owed).
fn card_ids(tree: &UiNode) -> Vec<String> {
    fn walk(n: &UiNode, out: &mut Vec<String>) {
        if n.id.starts_with("card_") {
            out.push(n.id.clone());
        }
        for c in &n.children {
            walk(c, out);
        }
    }
    let mut out = Vec::new();
    walk(tree, &mut out);
    out
}

/// Every two-way `bind` the authored tree carries — the fold-back set (rule
/// 3A04B4CE), DERIVED from the tree: the tree IS the list, so a new bound demo
/// control round-trips without touching Rust.
fn tree_binds(tree: &UiNode) -> Vec<String> {
    fn walk(n: &UiNode, out: &mut Vec<String>) {
        if let Some(b) = n.bind.as_deref() {
            if !b.is_empty() && !out.iter().any(|k| k == b) {
                out.push(b.to_string());
            }
        }
        for c in &n.children {
            walk(c, out);
        }
    }
    let mut out = Vec::new();
    walk(tree, &mut out);
    out
}

/// The viewport-card demo's framing radius and wireframe colours.
const DEMO_RADIUS: f32 = 2.2;
const DEMO_GROUND: [f32; 4] = [0.25, 0.28, 0.34, 1.0];
const DEMO_CUBE: [f32; 4] = [0.55, 0.75, 0.95, 1.0];

/// The viewport demo's content — a unit cube standing on the grid floor, as the 12
/// wireframe edges `Renderer::draw_lines` takes. One card, so rebuilding it a frame is free.
fn demo_cube() -> Vec<(Vec3, Vec3)> {
    let c = |x: f32, y: f32, z: f32| Vec3::new(x, y, z);
    let p = [
        c(-1.0, -1.0, -1.0),
        c(1.0, -1.0, -1.0),
        c(1.0, 1.0, -1.0),
        c(-1.0, 1.0, -1.0),
        c(-1.0, -1.0, 1.0),
        c(1.0, -1.0, 1.0),
        c(1.0, 1.0, 1.0),
        c(-1.0, 1.0, 1.0),
    ];
    vec![
        (p[0], p[1]),
        (p[1], p[2]),
        (p[2], p[3]),
        (p[3], p[0]),
        (p[4], p[5]),
        (p[5], p[6]),
        (p[6], p[7]),
        (p[7], p[4]),
        (p[0], p[4]),
        (p[1], p[5]),
        (p[2], p[6]),
        (p[3], p[7]),
    ]
}

// ───────────────────────────────────────────────────────────────────
// The three authored PAGES (B05B3D09 §4b/§4c/§4d)
// ───────────────────────────────────────────────────────────────────

/// **The MODALS page's bank**: one authored button per PARAM-DRIVEN shared tree,
/// paired with the id [`SharedModal::open`] resolves. The catalog is the EXERCISER of
/// the host seam (1F0F7347) — every tree the seam can host is reachable from here, and
/// the registry gate below fails by name when one is not.
///
/// `pause`, `confirm` and `settings` are deliberately absent, and the seam REFUSES
/// them ([`flicker_shell::modal_host_of`]): each is hosted by its own scene, whose
/// authored buttons fire names the param seam cannot map. Opening them from here was
/// incident B89FAC21 — a modal with no working control and no back-out, which Aaron
/// had to force-quit. A demo is not a reason to host a trap.
const MODAL_BUTTONS: &[(&str, &str)] = &[
    ("cat_modal_choice", "choice_dialog"),
    ("cat_modal_menu", "popup_menu"),
    ("cat_modal_prompt", "text_prompt"),
    ("cat_modal_busy", "busy"),
    ("cat_modal_conflict", flicker_shell::MODAL_CONFLICT),
];

/// The `rows_from` source the RECIPES page's breadcrumb expands from — the path is
/// DATA the bench publishes, so a deeper path is more rows and never more tree.
const CRUMB_SOURCE: &str = "cat_rec_crumbs";

/// The FILLERS page's `surface` node ids — the seats the walker reserves and the
/// bench fills. The pinned roster gate below walks exactly this list.
const PLOT_SPARK: &str = "cat_plot_spark";
const PLOT_BARS: &str = "cat_plot_bars";
const PLOT_CURVE: &str = "cat_plot_curve";
const GRAPH_SLOT: &str = "cat_graph";
const TIMELINE_SLOT: &str = "cat_timeline";
const GADGET_SLOT: &str = "cat_gadget";
const DOLL_SLOT: &str = "cat_doll";
/// The two authored 3D stages the last two filler cards render through.
const GADGET_STAGE: &str = "cat_gadget_stage";
const DOLL_STAGE: &str = "cat_doll_stage";

/// The demo timeline's length in frames, and how many lanes it rules.
const TIMELINE_FRAMES: u32 = 240;
const TIMELINE_LANES: usize = 7;
/// How many samples the demo plot series carries.
const SERIES_LEN: usize = 96;
/// How long the Busy modal's demo job runs, and in how many steps.
const BUSY_STEPS: u32 = 20;
const BUSY_STEP_MS: u64 = 100;
/// How long the demo job HOLDS at 100% before finishing — the window in which the
/// DISMISSABLE toggle (ruling DA0E1B57) is visibly the other way round. Cancel is
/// swallowed for the two seconds the bar is filling (`busy.lua` reads `modal_done`
/// false and holds the slab shut) and works during this hold, closing with `cancelled`;
/// wait it out and the handle finishes, closing with `done`. Both readouts land on the
/// Modals page's answer card, which is what makes the toggle a thing you can SEE.
const BUSY_HOLD_MS: u64 = 4000;

/// ONE authored colour out of this scene's own styles, as the rgba a surface filler
/// draws with — the five-line split at the filler seam (rule 790872EE: the palette
/// lives in `ui_theme.json`, the scene's `styles` block says which token each element
/// wears, and the Rust filler receives finished numbers and owns no colour). A path
/// that does not resolve warns and comes back TRANSPARENT: the element then draws
/// nothing, which is the loud answer rather than a stand-in nobody authored.
fn style_rgba(styles: &serde_json::Value, path: &str) -> [f32; 4] {
    let mut cur = styles;
    for seg in path.split('.') {
        match cur.get(seg) {
            Some(v) => cur = v,
            None => {
                tracing::warn!("componentcatalog: no style at `{path}` — that ink stays unset");
                return [0.0; 4];
            }
        }
    }
    match cur.as_array() {
        Some(a) if a.len() >= 4 => {
            let mut out = [0.0f32; 4];
            for (i, c) in a.iter().take(4).enumerate() {
                out[i] = c.as_f64().unwrap_or(0.0) as f32;
            }
            out
        }
        _ => {
            tracing::warn!("componentcatalog: style `{path}` is not an rgba array");
            [0.0; 4]
        }
    }
}

/// A seat only while the card is actually IN the tray's viewport.
///
/// The walker reserves a `surface` wherever the layout put it — a `list` does not cull
/// its children — so a card scrolled far below the fold still has a rect, out in the
/// margin. The HUD-drawn fillers are clipped to the tray and so do not care; the two
/// that composite an OFFSCREEN PASS would otherwise paint their image over the nav
/// rail. Unseating them out there is both the correct picture and the cheap one: an
/// unseated filler declares no pass at all.
fn seat_in_tray(slot: Option<&SurfaceSlot>, tray: Option<Rect>) -> Option<&SurfaceSlot> {
    let Some(t) = tray else { return slot };
    slot.filter(|s| s.y + s.h > t.pos.y && s.y < t.pos.y + t.size.y)
}

/// The demo series every Plot card reads — one shape, three readings, so the card
/// shows what the KIND changes and nothing else. Deterministic (no clock, no rng):
/// the headless extent gate walks the same curve the window shows.
fn demo_series() -> PlotSeries {
    let mut s = PlotSeries::new(SERIES_LEN);
    for i in 0..SERIES_LEN {
        let t = i as f32 / SERIES_LEN as f32;
        s.push((t * std::f32::consts::TAU * 1.5).sin() * 0.6 + t * 0.5);
    }
    s
}

/// The demo graph's THREE nodes and its edges — a chain plus one SELF-LOOP, which is
/// the case a state machine has and a dependency graph forbids (the filler draws the
/// loop as an arc above its own card; refusing one is the consumer's rule, not the
/// filler's).
fn demo_graph_edges() -> [GraphEdge; 3] {
    [
        GraphEdge {
            from: 0,
            to: 1,
            ink: EdgeInk::Idle,
        },
        GraphEdge {
            from: 1,
            to: 2,
            ink: EdgeInk::Lit,
        },
        GraphEdge {
            from: 2,
            to: 2,
            ink: EdgeInk::Selected,
        },
    ]
}

/// The demo timeline's content: one SPAN per lane at a staggered offset plus two
/// POINT events, so both event shapes and the lane ruling are on screen at once.
fn demo_timeline_events() -> Vec<TimelineEvent> {
    let mut out = Vec::new();
    for lane in 0..TIMELINE_LANES {
        let start = 12 + lane as u32 * 24;
        out.push(TimelineEvent {
            lane,
            start,
            end: Some(start + 40),
            selected: lane == 2,
        });
    }
    out.push(TimelineEvent {
        lane: 0,
        start: 190,
        end: None,
        selected: false,
    });
    out.push(TimelineEvent {
        lane: 4,
        start: 210,
        end: None,
        selected: false,
    });
    out
}

/// The UI test scene. Everything a frame needs lives here; the shell drives it through
/// the [`Scene`] trait.
pub struct ComponentCatalog {
    /// The component tree, parsed ONCE from `componentcatalog.scene.json`.
    tree: Option<UiNode>,
    /// The screen's declarative intents (S9) — `on_menu = "pause_open"`.
    ui_intents: UiIntents,
    /// Token-resolved `ui_theme.json` styles.
    ui_styles: serde_json::Value,
    /// Draw commands stashed by `update`'s walker pass, blitted in `render`.
    hud_commands: Vec<HudCommand>,
    /// The theme's engine textures in ID ORDER — for `render_hud` + the sprite card.
    textures: Vec<TextureHandle>,
    /// Theme for the pause overlay we push (built once in `enter`).
    theme: Option<Theme>,
    /// The retained walker [`UiState`] the nav writes its ONE focus id through.
    ui_state: UiState,
    /// The content tray's scroll offset (px). Set on a bookmark click (scroll-to), moved
    /// by the wheel, always adopted back clamped from the list's echo.
    content_scroll: f32,
    /// The card currently at the top of the tray — which bookmark highlights.
    section: usize,
    /// The tray's `card_<i>` ids in tree order — derived from the authored tree.
    cards: Vec<String>,
    /// Every two-way bind the tree carries — the fold-back set, derived likewise.
    binds: Vec<String>,
    /// Live values every bound control round-trips through (rule 3A04B4CE).
    /// Starts EMPTY — the PAIR SCRIPT seeds the demo values; a committed value
    /// echoed back here wins over the seed from then on.
    demo: ValueMap,
    /// The PAIR SCRIPT host (`componentcatalog.lua`).
    script: Option<ScriptHost>,
    /// The viewport card's shared filler — built LAZILY on first render (it needs
    /// `&mut Renderer`), then seated in the reserved rect each frame.
    viewport: Option<ViewportFiller>,
    /// The rect + layout the walker reserved for the `cat_surface` node this frame
    /// (captured in `update`; consumed in the input-less `render`). `None` when off screen.
    surface_seat: Option<(Rect, ViewportLayout)>,
    /// The walker's pointer SAMPLE for the card's surface this frame (the barrier) —
    /// `render` replays it to orbit the panel under the cursor; `render` gets no input
    /// of its own, and the bench never reads the device for it.
    pointer: Option<SurfacePointer>,

    // ── The FILLERS page (B05B3D09 §4d): one engine-crate filler per card, each
    //    seated on the plain `surface` the walker reserved for it. ──
    /// The one demo series all three Plot cards read.
    series: PlotSeries,
    /// The same series as a sparkline, as bars, and as a filled curve.
    plot_spark: Plot,
    plot_bars: Plot,
    plot_curve: Plot,
    /// The node-graph filler and its resolved ink.
    graph: GraphCanvas,
    canvas_style: CanvasStyle,
    /// The lane-timeline filler, its ink, and the per-lane colours (each lane's four
    /// slots come off `stat_dot.hues.<hue>`, so a lane and a legend dot share a token).
    timeline: Timeline,
    timeline_style: TimelineStyle,
    lane_styles: [LaneStyle; TIMELINE_LANES],
    /// The gadget overlay and the RigView panel it draws into.
    gadget: Gadget,
    gadget_view: RigView,
    gadget_style: GadgetStyle,
    /// The doll preview. The catalog uploads no rig (a rig is bench CONTENT), so this
    /// shows the doll's authored ground ring — the seat, the stage and the clock are
    /// the same ones a rigged consumer gets.
    doll: Doll,
    /// The bench's own clock (seconds), driving the doll's ring pulse — the demo
    /// motion the live cards need.
    clock: f32,
    /// The TRANSPORT recipe's play-head, in frames, and whether it is running. The
    /// Timeline filler's playhead reads the same number, so the recipe and the filler
    /// demonstrate one transport rather than two.
    tp_frame: u32,
    tp_playing: bool,

    // ── The MODALS page (B05B3D09 §4c): the SharedModal seam, exercised. ──
    /// The shared modal this frame asked to open, by registry id; consumed by the
    /// push below so a click opens exactly one modal.
    open_modal: Option<&'static str>,
    /// The last `modal_closed` answer — the result name and its payload — as the two
    /// readouts on the Modals page bind to.
    modal_result: String,
    modal_payload: String,
    /// The Busy card's live progress handle, kept so a second press replaces it.
    busy: Option<ModalProgress>,
    /// The last drop the DRAG → DROP recipe resolved (`payload id · target id`).
    drop_readout: String,
    /// The COLLAPSIBLE GROUP recipe's open flag — the header button flips it and the
    /// body cell's `visible_bind` reads it. Held here because a `visible_bind` is not
    /// a two-way `bind`: there is no control to echo it back.
    collapse_open: bool,
}

impl ComponentCatalog {
    /// A fresh catalog scene — takes its authored tree off the MANIFEST's def (the
    /// kernel parsed the file; this bench is only the behaviour that plays it) and
    /// seeds the demo values.
    pub fn new(def: &SceneDef) -> Self {
        let ui_styles = flicker::ui::load_shared_styles(def.styles.as_ref());
        let tree = def.tree.clone();
        if tree.is_none() {
            tracing::error!("scene '{}' declares no `tree` — no UI", def.id);
        }
        let ui_intents = tree.as_ref().map(UiIntents::of).unwrap_or_default();
        let cards = tree.as_ref().map(card_ids).unwrap_or_default();
        let binds = tree.as_ref().map(tree_binds).unwrap_or_default();

        // ── The FILLERS page's ink, resolved ONCE from this scene's own style blocks.
        //    Every colour below is a `$token` in `componentcatalog.scene.json`; not one
        //    rgba literal reaches a filler from here (rule 790872EE). ──
        let plot_style = PlotStyle {
            line: style_rgba(&ui_styles, "plot.line"),
            fill: style_rgba(&ui_styles, "plot.fill"),
            baseline: style_rgba(&ui_styles, "plot.baseline"),
            grid: style_rgba(&ui_styles, "plot.grid"),
            ..Default::default()
        };
        let canvas_style = CanvasStyle {
            bg: style_rgba(&ui_styles, "canvas.bg"),
            edge: style_rgba(&ui_styles, "canvas.edge"),
            edge_lit: style_rgba(&ui_styles, "canvas.edge_lit"),
            card_fill_top: style_rgba(&ui_styles, "canvas.card_fill_top"),
            card_fill_bot: style_rgba(&ui_styles, "canvas.card_fill_bot"),
            card_border: style_rgba(&ui_styles, "canvas.card_border"),
            card_border_selected: style_rgba(&ui_styles, "canvas.card_border_selected"),
            label: style_rgba(&ui_styles, "canvas.label"),
            label_selected: style_rgba(&ui_styles, "canvas.label_selected"),
            meta: style_rgba(&ui_styles, "canvas.meta"),
            icon_top: style_rgba(&ui_styles, "canvas.icon_top"),
            icon_bot: style_rgba(&ui_styles, "canvas.icon_bot"),
            icon_border: style_rgba(&ui_styles, "canvas.icon_border"),
            port: style_rgba(&ui_styles, "canvas.port"),
            link: style_rgba(&ui_styles, "canvas.link"),
        };
        let timeline_style = TimelineStyle {
            ruler: style_rgba(&ui_styles, "timeline.ruler"),
            tick: style_rgba(&ui_styles, "timeline.tick"),
            playhead: style_rgba(&ui_styles, "timeline.playhead"),
            event_selected: style_rgba(&ui_styles, "timeline.event_selected"),
        };
        // Each lane's ink IS a signal hue — the same `stat_dot.hues.<hue>` token the
        // legend recipe's dots wear, so a lane and its legend entry cannot drift apart.
        const LANE_HUES: [&str; TIMELINE_LANES] =
            ["blue", "green", "orange", "yellow", "red", "white", "black"];
        let row = style_rgba(&ui_styles, "timeline.row");
        let row_border = style_rgba(&ui_styles, "timeline.row_border");
        let lane_styles = LANE_HUES.map(|hue| LaneStyle {
            row,
            row_border,
            swatch: style_rgba(&ui_styles, &format!("stat_dot.hues.{hue}.fill")),
            event: style_rgba(&ui_styles, &format!("stat_dot.hues.{hue}.glow")),
        });
        let gadget_style = GadgetStyle {
            idle: [
                style_rgba(&ui_styles, "gadget.x"),
                style_rgba(&ui_styles, "gadget.y"),
                style_rgba(&ui_styles, "gadget.z"),
            ],
            aimed: style_rgba(&ui_styles, "gadget.aimed"),
            locked: style_rgba(&ui_styles, "gadget.locked"),
            modifying: style_rgba(&ui_styles, "gadget.modifying"),
            refused: style_rgba(&ui_styles, "gadget.refused"),
        };
        let gadget_view = RigView::new(GADGET_STAGE, &ui_styles, Projection::Perspective);
        let doll = Doll::new(DOLL_STAGE, &ui_styles);

        Self {
            tree,
            ui_intents,
            ui_styles,
            hud_commands: Vec::new(),
            textures: Vec::new(),
            theme: None,
            ui_state: UiState::default(),
            content_scroll: 0.0,
            section: 0,
            cards,
            binds,
            demo: ValueMap::new(),
            script: match ScriptHost::new(CATALOG_SCRIPT, "componentcatalog.lua") {
                Ok(h) => Some(h),
                Err(e) => {
                    tracing::error!("componentcatalog.lua failed to load — raw values only: {e}");
                    None
                }
            },
            viewport: None,
            surface_seat: None,
            pointer: None,
            series: demo_series(),
            plot_spark: Plot::new(PlotKind::Sparkline, plot_style),
            plot_bars: Plot::new(PlotKind::Bars, plot_style),
            plot_curve: Plot::new(PlotKind::Curve, plot_style),
            graph: GraphCanvas::new(CanvasMetrics::default()),
            canvas_style,
            timeline: Timeline::new(TimelineMetrics::default()),
            timeline_style,
            lane_styles,
            gadget: Gadget::default(),
            gadget_view,
            gadget_style,
            doll,
            clock: 0.0,
            tp_frame: 0,
            tp_playing: false,
            open_modal: None,
            modal_result: String::new(),
            modal_payload: String::new(),
            busy: None,
            drop_readout: String::new(),
            collapse_open: true,
        }
    }

    /// The RECIPES page's breadcrumb rows — the `rows_from` source the walker's
    /// prototype expands from. Three crumbs, each already-resolved display text, so
    /// the tree carries no literal and the path stays DATA.
    fn rows(&self, source: &str) -> Option<Vec<Row>> {
        (source == CRUMB_SOURCE).then(|| {
            [
                ("root", "$cat_rec_crumb_root"),
                ("mid", "$cat_rec_crumb_mid"),
                ("leaf", "$cat_rec_crumb_leaf"),
            ]
            .into_iter()
            .map(|(id, token)| Row::new(id, strings::resolve(token).into_owned()))
            .collect()
        })
    }

    /// The demo params each shared modal opens with. One place, so the Modals page's
    /// button, the modal's offer and the gate's expectation cannot drift apart; every
    /// caption is a `$token` the modal resolves once (never a literal in a tree).
    fn modal_params(&self, id: &str) -> ModalParams {
        let cancel = ModalOption::secondary("$cat_modal_cancel", "cat_modal_cancelled");
        match id {
            "popup_menu" => ModalParams::new()
                .title("$cat_modal_menu_title")
                .option(ModalOption::secondary("$cat_opt_alpha", "cat_modal_alpha"))
                .option(ModalOption::secondary("$cat_opt_bravo", "cat_modal_bravo"))
                .option(ModalOption::secondary(
                    "$cat_opt_charlie",
                    "cat_modal_charlie",
                ))
                .cancellable(cancel),
            "text_prompt" => ModalParams::new()
                .title("$cat_modal_prompt_title")
                .body("$cat_modal_prompt_body")
                .option(ModalOption::primary("$cat_modal_ok", "cat_modal_named"))
                .cancellable(cancel)
                .text(ModalText {
                    kind: String::new(),
                    initial: strings::resolve("$cat_rec_crumb_leaf").into_owned(),
                    max_len: 32,
                }),
            "busy" => ModalParams::new()
                .title("$cat_modal_busy_title")
                .body("$cat_modal_busy_body")
                .progress(self.busy.clone().unwrap_or_default()),
            flicker_shell::MODAL_CONFLICT => ModalParams::new()
                .title("$modal_conflict_title")
                .option(ModalOption::secondary("$cat_modal_skip", "cat_modal_skip"))
                .option(ModalOption::secondary(
                    "$cat_modal_keep_both",
                    "cat_modal_keep_both",
                ))
                .option(ModalOption::danger(
                    "$cat_modal_replace",
                    "cat_modal_replace",
                ))
                .cancellable(cancel)
                .conflict(ModalConflict {
                    name: strings::resolve("$cat_modal_conflict_name").into_owned(),
                    folder: strings::resolve("$cat_modal_conflict_folder").into_owned(),
                    existing: strings::resolve("$cat_modal_conflict_existing").into_owned(),
                    incoming: strings::resolve("$cat_modal_conflict_incoming").into_owned(),
                    remaining: 2,
                    apply_rest: false,
                }),
            // `choice_dialog` takes the plain demo offer: a title, a body and two
            // verbs. It is also the fallback for any tree added to the registry before
            // this page grows demo params of its own — the seam is ONE call for every
            // param-driven tree, which is what the roster gate below pins.
            _ => ModalParams::new()
                .title("$cat_modal_demo_title")
                .body("$cat_modal_demo_body")
                .option(ModalOption::primary("$cat_modal_ok", "cat_modal_ok"))
                .cancellable(cancel),
        }
    }

    /// **What the components read.** The engine publishes the RAW runtime
    /// variables (committed demo echoes, the tray section, the scroll offset);
    /// the PAIR SCRIPT derives everything else — seeds, the nav highlight, the
    /// Paged Menu gates (five-line split: logic lives in componentcatalog.lua).
    fn model(&self) -> ValueMap {
        let mut raw = self.demo.clone();
        raw.set(CONTENT_SCROLL_BIND, f64::from(self.content_scroll));
        raw.set("section", self.section as f64);
        raw.set("card_count", self.cards.len() as f64);
        // Q2 (ruling 7AB130A7): publish the live display device so the Paged Menu drops
        // its pad shoulder-glyph hints on kbm and shows them on a controller.
        raw.set(
            "input_device",
            flicker::input_device::last_input_context().token(),
        );
        // The binding-icon card reads LIVE bindings (bind_<sig>/glyph_<sig>) from the
        // profile's World map — the same channel every bench footer legend rides.
        flicker_shell::publish_signal_bindings(
            &mut raw,
            &flicker_shell::current_world_map(),
            [
                flicker_input_core::ActionSignal::Interact,
                flicker_input_core::ActionSignal::Menu,
                flicker_input_core::ActionSignal::Confirm,
            ],
        );
        // The three new pages' RAW runtime facts: the transport's play-head, the last
        // drop the walker's drag channel resolved, and the last answer the SharedModal
        // seam handed back. Each is runtime DATA (an identifier or a number), never
        // display copy — the words around them are `$token`s in the tree.
        raw.set(
            "cat_rec_tp_readout",
            format!("{} / {}", self.tp_frame, TIMELINE_FRAMES),
        );
        raw.set("cat_rec_tp_playing", self.tp_playing);
        raw.set("cat_rec_drop_readout", self.drop_readout.clone());
        raw.set("cat_rec_collapse_open", self.collapse_open);
        raw.set("cat_modal_result_val", self.modal_result.clone());
        raw.set("cat_modal_payload_val", self.modal_payload.clone());

        let mut m = raw.clone();
        if let Some(script) = &self.script {
            if let Err(e) = script.set_model(&raw) {
                tracing::error!("componentcatalog: publishing raw vars failed: {e}");
            }
            match script.derive() {
                Ok(Some(derived)) => {
                    for (k, v) in derived.entries() {
                        m.set(k.clone(), v.clone());
                    }
                }
                Ok(None) => {}
                Err(e) => tracing::error!("componentcatalog.lua derive() failed: {e}"),
            }
        }
        // The resource readout reads the gauge's DERIVED value (the pair script seeds
        // it, a committed drag replaces it), so the number and the bar can never
        // disagree — published after the derive for exactly that reason.
        let pct = (m.number("cat_rec_gauge_val").unwrap_or(0.0) * 100.0).round() as i64;
        m.set("cat_rec_res_readout", format!("{pct} / 100"));
        m
    }

    /// Fold each committed demo value so its control shows it next frame (rule 3A04B4CE);
    /// a control that did not move re-echoes its resting value, a no-op.
    fn apply_results(&mut self, results: &ValueMap) {
        for k in &self.binds {
            if let Some(v) = results.get(k) {
                self.demo.set(k.clone(), v.clone());
            }
        }
    }

    /// **The RECIPES page's live verbs**, folded from the ONE activation channel a
    /// click, a pad Confirm and a DROP all arrive on — the same `results` map every
    /// other control on the screen answers through.
    ///
    /// The DRAG → DROP recipe is the interesting one: the walker already resolved the
    /// release (or the pad Confirm on the focused target) against the target's
    /// `drop_accept`, fired its `drop_action` and published the payload id + target id
    /// on the drag channel. All the bench does is READ them — the walker never decides
    /// what a drop means, and this is what that division looks like in a consumer.
    fn fold_recipe_verbs(&mut self, results: &ValueMap) {
        if results.is_on("cat_rec_collapse_hdr") {
            self.collapse_open = !self.collapse_open;
        }
        if results.is_on("cat_rec_tp_play") {
            self.tp_playing = !self.tp_playing;
        }
        if results.is_on("cat_rec_tp_prev") {
            self.tp_frame = self.tp_frame.saturating_sub(12);
        }
        if results.is_on("cat_rec_tp_next") {
            self.tp_frame = (self.tp_frame + 12) % TIMELINE_FRAMES;
        }
        if results.is_on("cat_rec_dropped") {
            let id = results.text("drop_id").unwrap_or_default().to_string();
            let target = results.text("drop_target").unwrap_or_default().to_string();
            self.drop_readout = format!("{id} · {target}");
        }
    }

    /// **The three HUD-drawn fillers**, appended to this frame's command list inside
    /// the tray's scissor.
    ///
    /// Each one is seated on the rect the walker reserved, fed content the bench
    /// rebuilds each frame, and driven by the walker's pointer SAMPLE — the three
    /// contracts every surface filler in the engine honours. Colour never enters:
    /// the styles were resolved from this scene's own `$token` blocks in `new`.
    fn draw_fillers(
        &mut self,
        tray: Option<Rect>,
        middle: bool,
        graph_slot: Option<&SurfaceSlot>,
        timeline_slot: Option<&SurfaceSlot>,
        graph_pointer: Option<&SurfacePointer>,
        timeline_pointer: Option<&SurfacePointer>,
    ) {
        let sample = |p: Option<&SurfacePointer>| {
            let mut s = p.map(PointerSample::from).unwrap_or_default();
            s.middle = s.inside && middle;
            s
        };
        let mut out = Vec::new();

        // PLOT — one series, three readings. `commands` is empty while unseated.
        out.extend(self.plot_spark.commands(&self.series));
        out.extend(self.plot_bars.commands(&self.series));
        out.extend(self.plot_curve.commands(&self.series));

        // GRAPH CANVAS — three demo cards and one self-loop, pan/zoom live.
        if let Some(slot) = graph_slot {
            let keys = ["idle", "walk", "loop"];
            let edges = demo_graph_edges();
            self.graph.seat(Rect {
                pos: Vec2::new(slot.x, slot.y),
                size: Vec2::new(slot.w, slot.h),
            });
            self.graph.layout(&keys, &edges);
            self.graph.pointer(&sample(graph_pointer), &keys);
            let titles = [
                strings::resolve("$cat_fill_graph_n1").into_owned(),
                strings::resolve("$cat_fill_graph_n2").into_owned(),
                strings::resolve("$cat_fill_graph_n3").into_owned(),
            ];
            let metas = [
                strings::resolve("$cat_fill_graph_m1").into_owned(),
                strings::resolve("$cat_fill_graph_m2").into_owned(),
                strings::resolve("$cat_fill_graph_m3").into_owned(),
            ];
            let meta_lines = [
                [metas[0].as_str()],
                [metas[1].as_str()],
                [metas[2].as_str()],
            ];
            let nodes = [
                GraphNode {
                    title: &titles[0],
                    meta: &meta_lines[0],
                    selected: false,
                    icon: true,
                    ports: 1,
                },
                GraphNode {
                    title: &titles[1],
                    meta: &meta_lines[1],
                    selected: true,
                    icon: true,
                    ports: 1,
                },
                GraphNode {
                    title: &titles[2],
                    meta: &meta_lines[2],
                    selected: false,
                    icon: true,
                    ports: 1,
                },
            ];
            self.graph
                .draw(&nodes, &edges, &self.canvas_style, slot.layer, &mut out);
        }

        // TIMELINE — seven lanes, spans + points, the transport's play-head.
        if let Some(slot) = timeline_slot {
            let events = demo_timeline_events();
            self.timeline.seat(
                Rect {
                    pos: Vec2::new(slot.x, slot.y),
                    size: Vec2::new(slot.w, slot.h),
                },
                TIMELINE_LANES,
                TIMELINE_FRAMES,
            );
            self.timeline.set_playhead(self.tp_frame);
            self.timeline.pointer(&sample(timeline_pointer), &events);
            let labels: [String; TIMELINE_LANES] = std::array::from_fn(|i| {
                strings::resolve(&format!("$cat_fill_lane_{}", i + 1)).into_owned()
            });
            let lanes: Vec<TimelineLane> = labels
                .iter()
                .zip(self.lane_styles)
                .map(|(label, style)| TimelineLane { label, style })
                .collect();
            self.timeline
                .draw(&lanes, &events, &self.timeline_style, slot.layer, &mut out);
        }

        if out.is_empty() {
            return;
        }
        // The SCISSOR: a filler's commands are absolutely placed at the rect the
        // walker reserved, and the tray SCROLLS — so a card whose seat has slid out of
        // the list would otherwise paint over the nav rail. Clip to the tray's own
        // viewport and restore, exactly as the list's own content run does.
        match tray {
            Some(t) => {
                self.hud_commands.push(HudCommand::Clip {
                    rect: Some([t.pos.x, t.pos.y, t.size.x, t.size.y]),
                });
                self.hud_commands.append(&mut out);
                self.hud_commands.push(HudCommand::Clip { rect: None });
            }
            None => self.hud_commands.append(&mut out),
        }
    }

    /// **The GADGET card**: the overlay's Aim → Locked → Modify machine over an empty
    /// rig, in the modes the SCENE authored.
    ///
    /// `modes` is the Model value `componentcatalog.lua` publishes (`gadget_modes`
    /// names, per the filler's contract): the pair script owns the gate, the bench
    /// maps the names, and the filler never reads the Model. The drag produces
    /// [`GadgetDelta`](flicker_rigview::GadgetDelta)s the card deliberately does NOT
    /// apply — there is no document here to move, and what the card demonstrates is
    /// the handle STATE, which is the half a consumer has to draw correctly.
    fn aim_gadget(&mut self, pointer: Option<&SurfacePointer>, dt: f32, modes: &str) {
        self.gadget
            .set_modes(modes_from_names(modes.split_whitespace()));
        self.gadget.set_frame(Vec3::ZERO, Mat3::IDENTITY, 1.0);
        self.gadget_view.set_frame(Vec3::ZERO, 1.6);
        // While a handle is held the panel must NOT also orbit — one gesture, one
        // meaning. The rig view still gets the frame so its pass keeps running.
        let camera_pointer = pointer.filter(|_| !self.gadget.dragging());
        self.gadget_view
            .update(dt, camera_pointer, (0.0, 0.0, 0.0), None);
        let ray = self.gadget_view.ray_at(pointer);
        self.gadget.pick(Projection::Perspective, ray);
        match (pointer, ray) {
            (Some(p), Some(r)) if p.pressed => {
                self.gadget.begin(Projection::Perspective, r, None);
            }
            (Some(p), Some(r)) if p.left && self.gadget.dragging() => {
                self.gadget.update(r);
            }
            (Some(p), _) if !p.left => self.gadget.end(),
            (None, _) => self.gadget.end(),
            _ => {}
        }
        let overlay = self
            .gadget
            .handle_lines(Projection::Perspective, &self.gadget_style);
        self.gadget_view.set_overlay(overlay);
    }

    /// The card whose top is nearest the tray's current scroll offset — the one at the top
    /// of the view, so its bookmark highlights. `card_i.y - card_0.y` is the height stacked
    /// above card `i`; the active card is the one whose stack ≈ the offset.
    fn active_card(&self, rects: &[(String, [f32; 4])], scroll: f32, fallback: usize) -> usize {
        let card_y = |id: &str| rects.iter().find(|(n, _)| n == id).map(|(_, r)| r[1]);
        let Some(first) = self.cards.first().and_then(|id| card_y(id)) else {
            return fallback;
        };
        let mut best = (f32::MAX, fallback);
        for (i, id) in self.cards.iter().enumerate() {
            if let Some(ci) = card_y(id) {
                let d = ((ci - first) - scroll).abs();
                if d < best.0 {
                    best = (d, i);
                }
            }
        }
        best.1
    }
}

impl Scene for ComponentCatalog {
    /// **The SharedModal seam, read back.** Every shared tree the Modals page opens
    /// closes on [`Transition::CloseModal`], and the kernel hands that answer to the
    /// scene the pop REVEALS — here. The catalog prints it verbatim: the result is the
    /// CALLER'S OWN action name (the fixed `modal_opt_<n>` slot names never leave the
    /// shell) and the payload is whatever that tree collects — a picked path, a typed
    /// name, the conflict checkbox's `1`/`0`. That is the whole contract, on screen.
    fn modal_closed(&mut self, modal: &str, result: &str, payload: Option<&str>) {
        self.modal_result = format!("{modal} · {result}");
        self.modal_payload = payload
            .filter(|p| !p.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| strings::resolve("$cat_modal_none").into_owned());
        // The demo job is over the moment its modal is gone — a second press wants a
        // fresh handle, not the finished one.
        self.busy = None;
    }

    /// The text_field card owns the keyboard while its session is open.
    fn input_context(&self) -> Option<InputContext> {
        self.ui_state
            .text_entry()
            .then_some(InputContext::TextEntry)
    }

    fn enter(&mut self, renderer: &mut Renderer) {
        renderer.clear_color = [0.02, 0.03, 0.05, 1.0];
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
        let Some(tree) = self.tree.as_ref() else {
            return Transition::None;
        };
        let screen = renderer.size();
        let dtf = dt.as_secs_f32();
        self.clock += dtf;
        // The transport recipe's play-head, on the time canon's 60 Hz clip rate. The
        // Timeline filler's playhead reads the same number below.
        if self.tp_playing {
            self.tp_frame = (self.tp_frame + (dtf * 60.0).round() as u32) % TIMELINE_FRAMES;
        }
        let mut model = self.model();
        // The RECIPES page's breadcrumb: the authored `row` carries ONE button
        // prototype and `rows_from`, and this expands it into the path's crumbs. The
        // tree stays static — the expansion is a per-frame VALUE, exactly as the
        // shared file browser's own breadcrumb works.
        let tree = instantiate_rows(tree, &mut model, &|source| self.rows(source));
        let tree = &tree;

        // ONE walker pass: layout + hit-test + draw the tray.
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
        let frame = run_ui(tree, &model, &self.ui_styles, &snap, &mut self.ui_state);
        // The viewport card: capture the rect+layout the walker reserved and this frame's
        // pointer sample, for the input-less `render` to seat + orbit (a left-drag over a
        // panel orbits it; the filler self-gates to the cursor-over-viewport).
        self.surface_seat = frame.surface_slot("cat_surface");
        // …and the walker's pointer SAMPLE for the surface (the barrier, A8C9F02B §4b):
        // present only while the cursor is over the card's well with no UI over it, or
        // while a press that began there is still held — never a device read.
        self.pointer = frame.surface_pointer("cat_surface").cloned();

        // ── The FILLERS page (§4d): every filler takes the seat the walker reserved
        //    for its `surface` node and the pointer SAMPLE the barrier handed that
        //    surface — never the device, never the screen size.
        //
        //    The tray SCROLLS and a `list` does not cull, so a card below the fold
        //    still has a reserved rect out in the margin: the HUD-drawn fillers are
        //    clipped to the tray's viewport, and the two that composite an offscreen
        //    pass are unseated outside it (see `seat_in_tray`). ──
        let tray_rect = frame.rect(CONTENT_SCROLL_BIND);
        self.plot_spark.seat(frame.surface(PLOT_SPARK));
        self.plot_bars.seat(frame.surface(PLOT_BARS));
        self.plot_curve.seat(frame.surface(PLOT_CURVE));
        let graph_slot = frame.surface(GRAPH_SLOT).cloned();
        let timeline_slot = frame.surface(TIMELINE_SLOT).cloned();
        self.gadget_view
            .seat(seat_in_tray(frame.surface(GADGET_SLOT), tray_rect));
        self.doll
            .seat(seat_in_tray(frame.surface(DOLL_SLOT), tray_rect));
        let graph_pointer = frame.surface_pointer(GRAPH_SLOT).cloned();
        let timeline_pointer = frame.surface_pointer(TIMELINE_SLOT).cloned();
        let gadget_pointer = frame.surface_pointer(GADGET_SLOT).cloned();

        let hud_hit = frame.results.is_on("hud_hit");
        // Take the frame apart: the draw commands to blit, the result values to read, and
        // the per-card RECTS the scroll-to needs (each card box reports its resolved Y).
        self.hud_commands = frame.commands;
        let mut results = frame.results;
        let rects = frame.rects;

        // The three HUD-drawn fillers, appended AFTER the walker's own commands (so
        // they land over their card's well) and inside the tray's scissor.
        let gadget_modes = model
            .text("cat_gadget_modes")
            .unwrap_or_default()
            .to_string();
        self.draw_fillers(
            tray_rect,
            input.mouse_middle,
            graph_slot.as_ref(),
            timeline_slot.as_ref(),
            graph_pointer.as_ref(),
            timeline_pointer.as_ref(),
        );
        self.aim_gadget(gadget_pointer.as_ref(), dtf, &gadget_modes);
        self.doll.tick(dtf);
        // A ring that pulses is the honest "this pass is live" tell on a doll with no
        // rig: the stage's `color_active` is what `active` selects.
        self.doll.set_live(true);
        self.doll.set_active(self.clock.rem_euclid(2.4) < 1.2);

        // ── The input seam (input-P3): dispatch the pump's events through the walker (with
        // nav); a fired nav/action folds into `results` exactly like a click. ──
        let mut walker = WalkerHandler::hud(&mut self.ui_state, hud_hit)
            .with_nav(tree, &model)
            .with_intents(&self.ui_intents);
        {
            let mut chain: [&mut dyn InputHandler; 1] = [&mut walker];
            Router::dispatch(signals.events, &mut chain, signals.route);
        }
        for name in walker.take_fired() {
            results.set(name.as_str(), true);
        }

        // The wheel wrote (and the list echoed, clamped) the tray offset — adopt it, so
        // the scene's copy tracks the list's clamped truth.
        if let Some(v) = results.number(CONTENT_SCROLL_BIND) {
            self.content_scroll = v as f32;
        }
        // A bookmark fired — scroll the tray so that card sits at the top. The offset is
        // the height stacked above it, read live from the layout (`card_i.y - card_0.y`),
        // so it is correct at any current scroll and for any card height. Overrides the
        // wheel adopt above (an explicit jump outranks the resting echo — rule 3A04B4CE).
        let card_y = |id: &str| rects.iter().find(|(n, _)| n == id).map(|(_, r)| r[1]);
        let jump = self
            .cards
            .iter()
            .find(|card| results.is_on(&format!("nav_{}", &card["card_".len()..])))
            .cloned();
        if let (Some(c0), Some(card)) = (self.cards.first().and_then(|id| card_y(id)), jump) {
            if let Some(ci) = card_y(&card) {
                self.content_scroll = (ci - c0).max(0.0);
            }
        }
        self.apply_results(&results);
        // Which card is at the top now → which bookmark highlights next frame.
        self.section = self.active_card(&rects, self.content_scroll, self.section);

        self.fold_recipe_verbs(&results);

        // ── The MODALS page: a button ARMS the shared tree it names; the push happens
        //    once, below, so a click opens exactly one modal. ──
        for (action, id) in MODAL_BUTTONS {
            if results.is_on(action) {
                self.open_modal = Some(id);
            }
        }
        if let (Some(id), Some(theme)) = (self.open_modal.take(), self.theme) {
            // Busy is the one modal that needs a WORKER: the host scene is frozen
            // beneath a modal, so it cannot publish a fraction per frame. The demo job
            // is a thread holding the other end of the shared handle — the shape a real
            // bake or scan uses — and `finish()` closes the modal with `done`.
            //
            // It also demonstrates the DISMISSABLE toggle (DA0E1B57). This modal
            // declares NO cancel option, so while the bar fills `busy.lua` holds the
            // slab shut and Esc / pad-B do nothing — try it. At 100% the job HOLDS
            // ([`BUSY_HOLD_MS`]): the work is done, the slab lets go, and a Cancel now
            // closes it with `cancelled`. Wait instead and `finish()` closes it with
            // `done`. The answer card below reads back whichever happened.
            if id == "busy" {
                let progress = ModalProgress::new();
                let worker = progress.clone();
                std::thread::spawn(move || {
                    for step in 1..=BUSY_STEPS {
                        std::thread::sleep(Duration::from_millis(BUSY_STEP_MS));
                        worker.set(step as f32 / BUSY_STEPS as f32);
                    }
                    std::thread::sleep(Duration::from_millis(BUSY_HOLD_MS));
                    worker.finish();
                });
                self.busy = Some(progress);
            }
            return Transition::Push(Box::new(SharedModal::open(
                theme,
                id,
                self.modal_params(id),
            )));
        }

        // Menu opens the shell's pause overlay (the screen's DECLARED `on_menu`).
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
        Transition::None
    }

    fn render<'f>(&'f mut self, renderer: &mut Renderer, fg: &mut FrameGraph<'f>) {
        let base_layer = fg.base_layer();
        // The viewport card. The `ViewportFiller` declares its offscreen passes +
        // composite into the frame's shared graph (the manager executes it once); seat
        // the shared filler in the rect the walker reserved, orbit the panel under the
        // cursor on a left-drag, and render a wireframe stage (ground grid + a cube) into
        // it — the kind's LIVE catalog exerciser. Wheel is left out so it never fights the
        // tray scroll. Composites at `base+2`, ABOVE the HUD's `base+1`, so the views land
        // inside the card's well.
        if let Some((rect, layout)) = self.surface_seat {
            if self.viewport.is_none() {
                self.viewport = Some(ViewportFiller::new(renderer, layout));
            }
            let vf = self.viewport.as_mut().unwrap();
            vf.set_rect(rect);
            if let Some(p) = &self.pointer {
                let orbit = p.captured && p.left;
                vf.apply_pointer(
                    p.cursor,
                    renderer.size(),
                    p.delta,
                    orbit,
                    false,
                    0.0,
                    DEMO_RADIUS,
                );
            }
            let ground = grid_segments_xy(0.5, 2.5, -1.0);
            let cube = demo_cube();
            vf.declare(
                renderer,
                fg,
                base_layer + 2.0,
                DEMO_RADIUS,
                move |r, _view| {
                    r.draw_lines(&ground, DEMO_GROUND);
                    r.draw_lines(&cube, DEMO_CUBE);
                },
            );
        }
        // The two 3D filler cards declare their own offscreen passes into the same
        // shared graph. An unseated card (the tray scrolled past it) declares nothing,
        // and the doll's per-surface clock skips a pass it does not need — so a page of
        // fillers costs what is actually on screen.
        self.gadget_view.render(renderer, fg, base_layer + 2.0);
        self.doll.render(renderer, fg, base_layer + 2.0);

        // The HUD is the screen surface's final 2D — one overlay after the composites.
        if let Some(&white) = self.textures.first() {
            let hud_commands = &self.hud_commands;
            let textures = &self.textures;
            fg.overlay(move |r| {
                r.set_layer(base_layer + 1.0);
                render_hud(r, hud_commands, white, textures);
                r.set_layer(base_layer);
            });
        }
    }

    /// Give the two 3D filler cards' render targets back. A `RenderTargetHandle` is an
    /// INDEX into the renderer's slot pool, so dropping the bench reclaims nothing —
    /// leaving without this strands a target per card the session ever showed
    /// (incident 5C9C27E1, rule 728E682F).
    fn exit(&mut self, renderer: &mut Renderer) {
        self.gadget_view.free(renderer);
        self.doll.release(renderer);
    }
}

/// The bench's launchable-scene factory — the CLIENT BEHAVIOUR the roster registers:
/// the manifest resolves `componentcatalog.scene.json` by id and hands its def here.
pub fn scene(def: &SceneDef) -> Box<dyn Scene> {
    Box::new(ComponentCatalog::new(def))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shipped scene file, read by the gates below exactly as the manifest reads it.
    const CATALOG_SCENE: &str =
        include_str!("../../../../content/sensorium/scenes/componentcatalog.scene.json");

    /// The nav's active/idle bookmark styles — the TEST-side mirror of the paths
    /// componentcatalog.lua publishes through each `style_bind` (the Lua owns the
    /// runtime choice; these pin it).
    const NAV_ACTIVE_STYLE: &str = "modal.buttons.variants.primary";
    const NAV_IDLE_STYLE: &str = "modal.buttons.variants.secondary";

    /// THE PAIR-SCRIPT REGRESSION GATE: componentcatalog.lua loads; derive()
    /// seeds the demo values, lights the active bookmark, and gates the Paged
    /// Menu card — the scene's component logic lives in the pair script.
    #[test]
    fn the_pair_script_seeds_and_derives() {
        let def = SceneDef::parse("componentcatalog", CATALOG_SCENE)
            .expect("componentcatalog.scene.json loads");
        let cat = ComponentCatalog::new(&def);
        assert!(
            cat.script.is_some(),
            "componentcatalog.lua loads (the pair script)"
        );
        let m = cat.model();
        assert!(m.is_on("cat_check_val"), "the checkbox demo seeds ON");
        assert_eq!(
            m.text("nav_sty_0"),
            Some(NAV_ACTIVE_STYLE),
            "bookmark 0 starts active"
        );
        assert_eq!(m.text("nav_sty_1"), Some(NAV_IDLE_STYLE), "the rest rest");
        assert!(
            m.is_on("cat_pm_on_p0"),
            "the Paged Menu card opens on page 1"
        );
    }

    /// THE AUTHORED-STYLE-PATH GATE (S1 of the styling pass): every style path any
    /// shipped scene tree names must resolve to a style BLOCK in that scene's
    /// merged styles. A path that resolves to nothing draws compiled defaults
    /// SILENTLY — the walker cannot warn (the lookup simply misses), so a card
    /// that claims to demo styling through a dead path is a lie nobody sees. The
    /// day it was written this gate caught five dead paths in the catalog and the
    /// sablework root slab.
    /// **Every focus-group member carries an `id`** (incident A0D3CE6A, Populous
    /// 2026-09-04): the walker makes a node focusable only when BOTH `tab_group` and
    /// `id` are non-empty, so a button that authors a group and an ordinal but no id is
    /// reachable by the mouse and invisible to the pad — six Populous buttons sat that
    /// way with every other gate green. Cross-scene, because the fault is authoring,
    /// not one bench's.
    #[test]
    fn every_tab_group_member_has_an_id() {
        fn collect(node: &serde_json::Value, scene: &str, out: &mut Vec<String>) {
            let has = |k: &str| node.get(k).and_then(|v| v.as_str()).is_some_and(|s| !s.is_empty());
            if has("tab_group") && !has("id") {
                out.push(format!(
                    "{scene}: `{}` in group `{}` (action {:?}, ordinal {:?}) has no id",
                    node.get("component").and_then(|c| c.as_str()).unwrap_or("?"),
                    node.get("tab_group").and_then(|g| g.as_str()).unwrap_or("?"),
                    node.get("action").and_then(|a| a.as_str()),
                    node.get("nav_ordinal"),
                ));
            }
            if let Some(kids) = node.get("children").and_then(|c| c.as_array()) {
                for kid in kids {
                    collect(kid, scene, out);
                }
            }
        }
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../content/sensorium/scenes");
        let mut idless = Vec::new();
        let mut seen = 0;
        for folder in [dir.clone(), dir.join("shared")] {
            for entry in std::fs::read_dir(&folder).expect("scenes folder reads") {
                let p = entry.expect("dir entry").path();
                if !p
                    .file_name()
                    .is_some_and(|n| n.to_string_lossy().ends_with(".scene.json"))
                {
                    continue;
                }
                let id = p.file_name().expect("name").to_string_lossy().to_string();
                let doc: serde_json::Value =
                    serde_json::from_str(&std::fs::read_to_string(&p).expect("scene reads"))
                        .unwrap_or_else(|e| panic!("{id} parses: {e}"));
                if let Some(tree) = doc.get("tree") {
                    seen += 1;
                    collect(tree, &id, &mut idless);
                }
            }
        }
        assert!(seen > 0, "the sweep saw scene trees");
        assert!(
            idless.is_empty(),
            "focus-group members the pad can never reach (no `id`):\n{}",
            idless.join("\n")
        );
    }

    #[test]
    fn every_authored_style_path_resolves_to_a_block() {
        const BLOCK_PROPS: [&str; 9] = [
            "style",
            "style_off",
            "panel_style",
            "divider_style",
            "rule_style",
            "glyph_style",
            "tab_active",
            "tab_idle",
            "runes_style",
        ];
        fn jwalk<'v>(root: &'v serde_json::Value, path: &str) -> Option<&'v serde_json::Value> {
            path.split('.').try_fold(root, |v, seg| v.get(seg))
        }
        // Raw-JSON walk, not SceneDef: the shared modal trees (scenes/shared/)
        // carry no `behaviour` — the manifest skips that folder and a host scene
        // merges them — but their authored paths must resolve all the same.
        fn collect(node: &serde_json::Value, out: &mut Vec<(String, String)>) {
            let kind = node
                .get("component")
                .and_then(|c| c.as_str())
                .unwrap_or("?");
            for prop in BLOCK_PROPS {
                if let Some(path) = node.get(prop).and_then(|p| p.as_str()) {
                    out.push((kind.to_string(), path.to_string()));
                }
            }
            if let Some(kids) = node.get("children").and_then(|c| c.as_array()) {
                for kid in kids {
                    collect(kid, out);
                }
            }
        }

        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../content/sensorium/scenes");
        let mut files: Vec<std::path::PathBuf> = Vec::new();
        for folder in [dir.clone(), dir.join("shared")] {
            for entry in std::fs::read_dir(&folder).expect("scenes folder reads") {
                let p = entry.expect("dir entry").path();
                if p.file_name()
                    .is_some_and(|n| n.to_string_lossy().ends_with(".scene.json"))
                {
                    files.push(p);
                }
            }
        }
        assert!(!files.is_empty(), "the scenes folder holds scene files");

        let mut broken = Vec::new();
        for path in files {
            let id = path
                .file_name()
                .expect("file name")
                .to_string_lossy()
                .trim_end_matches(".scene.json")
                .to_string();
            let text = std::fs::read_to_string(&path).expect("scene file reads");
            let doc: serde_json::Value = serde_json::from_str(&text)
                .unwrap_or_else(|e| panic!("{id}.scene.json parses: {e}"));
            let Some(tree) = doc.get("tree") else {
                continue;
            };
            // A shared modal tree renders under its HOST's merge (today: Main
            // merges scenes/shared/* back via main_scene_styles), so its paths
            // resolve against host styles ⊕ its own — that hosting contract is
            // part of what this gate pins.
            let in_shared = path.parent().is_some_and(|p| p.ends_with("shared"));
            let own = doc
                .get("styles")
                .cloned()
                .unwrap_or(serde_json::Value::Null);
            let scene_styles = if in_shared {
                let main_text = std::fs::read_to_string(dir.join("Main.scene.json"))
                    .expect("the host scene file reads");
                let main_doc: serde_json::Value =
                    serde_json::from_str(&main_text).expect("Main.scene.json parses");
                let mut merged = main_doc.get("styles").cloned().unwrap_or_default();
                if let (Some(m), Some(o)) = (merged.as_object_mut(), own.as_object()) {
                    for (k, v) in o {
                        m.insert(k.clone(), v.clone());
                    }
                }
                merged
            } else {
                own
            };
            let styles = flicker::ui::load_shared_styles(Some(&scene_styles));
            let mut named = Vec::new();
            collect(tree, &mut named);
            for (kind, p) in named {
                match jwalk(&styles, &p) {
                    Some(v) if v.is_object() => {}
                    Some(_) => broken.push(format!(
                        "{id}: {kind} style '{p}' resolves to a non-block value"
                    )),
                    None => broken.push(format!("{id}: {kind} names style '{p}' → NOTHING")),
                }
            }
        }
        assert!(
            broken.is_empty(),
            "authored style paths must resolve to blocks:\n{}",
            broken.join("\n")
        );
    }

    /// THE LADDER-PAIRING GATE (styling S2): a `size_class` button inside a ROW
    /// flows horizontally, so its measured rung height only lands when the row
    /// aligns non-stretch — a stretch row overrides every child's cross extent
    /// and the rung would be a silent no-op. Vertical flows (cell/panel/list/
    /// stack/popup_panel) consume the height as the MAIN extent and need nothing.
    #[test]
    fn every_ladder_button_in_a_row_sits_in_an_aligned_flow() {
        fn walk(
            node: &serde_json::Value,
            parent: Option<&serde_json::Value>,
            id: &str,
            broken: &mut Vec<String>,
        ) {
            let is_ladder_button = node.get("component").and_then(|c| c.as_str()) == Some("button")
                && node.get("size_class").is_some()
                && node.get("height").is_none();
            if is_ladder_button {
                if let Some(p) = parent {
                    let p_kind = p.get("component").and_then(|c| c.as_str()).unwrap_or("");
                    let p_align = p.get("align").and_then(|a| a.as_str()).unwrap_or("stretch");
                    if p_kind == "row" && p_align == "stretch" {
                        let bid = node.get("id").and_then(|i| i.as_str()).unwrap_or("<anon>");
                        broken.push(format!(
                            "{id}: button '{bid}' has a size_class inside a STRETCH row — the rung is a no-op; align the row (center/start/end)"
                        ));
                    }
                }
            }
            if let Some(kids) = node.get("children").and_then(|c| c.as_array()) {
                for kid in kids {
                    walk(kid, Some(node), id, broken);
                }
            }
        }

        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../content/sensorium/scenes");
        let mut broken = Vec::new();
        for folder in [dir.clone(), dir.join("shared")] {
            for entry in std::fs::read_dir(&folder).expect("scenes folder reads") {
                let p = entry.expect("dir entry").path();
                if !p
                    .file_name()
                    .is_some_and(|n| n.to_string_lossy().ends_with(".scene.json"))
                {
                    continue;
                }
                let id = p.file_name().expect("name").to_string_lossy().to_string();
                let doc: serde_json::Value =
                    serde_json::from_str(&std::fs::read_to_string(&p).expect("scene reads"))
                        .unwrap_or_else(|e| panic!("{id} parses: {e}"));
                if let Some(tree) = doc.get("tree") {
                    walk(tree, None, &id, &mut broken);
                }
            }
        }
        assert!(
            broken.is_empty(),
            "ladder buttons need an aligned row:\n{}",
            broken.join("\n")
        );
    }

    /// PROOF the catalog is NOT hardcoded — the full pipeline, walked end to end
    /// with the REAL shipped data, read-only (five-line architecture):
    ///
    /// 1. `ui_theme.json` = COLORS ONLY (no modal block); the modal layout details
    ///    live in THIS SCENE'S OWN file (`componentcatalog.scene.json` `styles`);
    /// 2. the bench's real loader path (`load_shared_styles(def.styles)`)
    ///    resolves `modal.buttons.variants.primary` from the scene's blocks;
    /// 3. the resolved fill equals the theme file's `theme.tokens.sap_base` — the
    ///    COLOUR comes from the one palette (the scene points, the theme defines);
    /// 4. `run_ui` over the REAL catalog tree emits a Panel draw command carrying
    ///    exactly that fill on the active nav bookmark — resolved live, per frame,
    ///    never baked into the bench.
    #[test]
    fn the_nav_rail_draws_rust_owned_modal_chrome_not_hardcoded_bytes() {
        // 1 — provenance: theme = colors only; the scene file owns its layout blocks.
        let theme: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(flicker::ui::shared_theme_path()).expect("theme reads"),
        )
        .expect("theme parses");
        assert!(
            theme.get("modal").is_none(),
            "ui_theme.json carries colors, nothing else"
        );
        let def = SceneDef::parse("componentcatalog", CATALOG_SCENE)
            .expect("componentcatalog.scene.json loads");
        assert!(
            def.styles.as_ref().and_then(|st| st.get("modal")).is_some(),
            "this scene's OWN file carries the modal layout details it uses"
        );

        // 2 + 3 — the live loader resolves the scene's blocks against the file palette.
        let styles = flicker::ui::load_shared_styles(def.styles.as_ref());
        let fill = styles["modal"]["buttons"]["variants"]["primary"]["fill_top"]
            .as_array()
            .expect("primary fill_top resolved to an rgba array (style satellite merge)")
            .iter()
            .map(|v| v.as_f64().unwrap() as f32)
            .collect::<Vec<f32>>();
        let raw_theme: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(flicker::ui::shared_theme_path()).expect("theme file reads"),
        )
        .expect("theme file parses");
        let sap_base = raw_theme["theme"]["tokens"]["sap_base"]
            .as_array()
            .expect("$sap_base lives in the FILE palette")
            .iter()
            .map(|v| v.as_f64().unwrap() as f32)
            .collect::<Vec<f32>>();
        assert_eq!(
            fill, sap_base,
            "structure from Rust, ink from the one palette"
        );

        // 4 — the REAL tree draws it: the active bookmark's slab carries the fill.
        let tree = SceneDef::parse("componentcatalog", CATALOG_SCENE)
            .expect("componentcatalog.scene.json loads")
            .tree
            .expect("it declares a tree");
        let mut model = ValueMap::new();
        model.set("nav_sty_0", NAV_ACTIVE_STYLE.to_string());
        let snap = UiInput {
            mouse: flicker::render::Vec2::new(-1.0, -1.0),
            clicked: false,
            down: false,
            right_down: false,
            screen: flicker::render::Vec2::new(1920.0, 1080.0),
            wheel: 0.0,
            exclusive: false,
            motion: Default::default(),
        };
        let frame = run_ui(&tree, &model, &styles, &snap, &mut UiState::default());
        let want = [fill[0], fill[1], fill[2], fill[3]];
        assert!(
            frame.commands.iter().any(|c| matches!(
                c,
                HudCommand::Panel { color, .. } if color.iter().zip(want.iter()).all(|(a, b)| (a - b).abs() < 1e-6)
            )),
            "the walked catalog tree draws the Rust-owned primary slab fill {want:?}"
        );
    }

    /// **Every pane group has a clear panel-navigation layer** (nav-tier contract
    /// 1B5F6BB8, flattened per 1A292918): a `tab_group` with interior controls must have
    /// exactly ONE actionless CONTAINER (with a non-zero authored ordinal) whose id equals
    /// the group, so a panel stop is a real container and never a bare leaf. The flatten
    /// lets BOTH the stick and the d-pad move between those stops; `Confirm` still scopes
    /// into a container's interior. This is the "ambiguous panel navigation is a violation"
    /// rule as a gate — `cat_content` was a container-less group (nav deposited the cursor
    /// on a leaf inside it), which this forbids.
    #[test]
    fn every_pane_group_has_a_clear_container() {
        let tree = SceneDef::parse("componentcatalog", CATALOG_SCENE)
            .expect("componentcatalog.scene.json loads")
            .tree
            .expect("it declares a tree");
        // Every node's (id, tab_group, nav_ordinal, has_action) — membership AND
        // the container candidates the ownership rule derives (nested panes,
        // Aaron 2026-08-15: a container is a node other nodes CLAIM as their
        // `tab_group`; it carries NO self-membership marker).
        fn collect(n: &UiNode, out: &mut Vec<(String, String, u32, bool)>) {
            if !n.id.is_empty() {
                out.push((
                    n.id.clone(),
                    n.tab_group.clone(),
                    n.nav_ordinal,
                    n.action.is_some(),
                ));
            }
            for c in &n.children {
                collect(c, out);
            }
        }
        let mut nodes = Vec::new();
        collect(&tree, &mut nodes);
        let groups: std::collections::BTreeSet<&str> = nodes
            .iter()
            .filter(|(_, g, _, _)| !g.is_empty())
            .map(|(_, g, _, _)| g.as_str())
            .collect();
        assert!(
            groups.contains("cat_nav") && groups.contains("cat_content"),
            "both panes present"
        );
        for g in groups {
            // OWNERSHIP: exactly one actionless node whose id IS the group — the
            // container Confirm enters. An unclaimed group would strand its
            // members off the pad (the fail-to-nothing class).
            let containers: Vec<_> = nodes
                .iter()
                .filter(|(id, _, _, act)| id == g && !act)
                .collect();
            assert_eq!(
                containers.len(),
                1,
                "pane group `{g}` needs exactly one actionless container node with that id",
            );
            // The stick-stop order is AUTHORED: every container carries an
            // explicit non-zero ordinal (Aaron 2026-08-15 — never tree-implicit).
            assert!(
                containers[0].2 > 0,
                "container `{g}` must author its stick-stop `nav_ordinal`",
            );
            for (id, grp, ord, _) in &nodes {
                if grp == g {
                    assert!(
                        *ord > 0,
                        "member `{id}` of `{g}` must author its ring ordinal"
                    );
                }
            }
        }
    }

    /// The shipped tree names only kinds the engine knows (S10 vocabulary gate) and ships
    /// NO raw display literal — every label is a `$token`, in the tree or a bound value
    /// this source publishes (the strings gate + its model-publish twin).
    #[test]
    fn the_scene_ships_clean_kinds_and_no_raw_literals() {
        let tree = SceneDef::parse("componentcatalog", CATALOG_SCENE)
            .expect("componentcatalog.scene.json loads")
            .tree
            .expect("it declares a tree");
        assert!(
            flicker::ui::unknown_kinds(&tree).is_empty(),
            "unknown component kinds: {:?}",
            flicker::ui::unknown_kinds(&tree)
        );
        assert!(
            flicker::ui::raw_display_literals(&tree).is_empty(),
            "raw display literals in the tree (need a $token): {:?}",
            flicker::ui::raw_display_literals(&tree)
        );
        let flags = flicker::ui::strings::raw_model_publish_literals(include_str!("lib.rs"));
        assert!(
            flags.is_empty(),
            "raw model-published copy (need a $token): {flags:?}"
        );
    }

    /// One bookmark + one card box per component, counts agreeing: [`CARD_COUNT`] and the
    /// tree's `nav_<i>` bookmarks + `card_<i>` boxes.
    #[test]
    fn one_bookmark_and_card_box_per_component() {
        let def = SceneDef::parse("componentcatalog", CATALOG_SCENE).expect("scene parses");
        let tree = def.tree.expect("the scene ships a tree");

        // Structure: every card has its bookmark and the ordinals chain 1..n in
        // authoring order (the drifted-ordinal class of bug fails here).
        let cards = card_ids(&tree);
        assert!(!cards.is_empty(), "the tray carries cards");
        for card in &cards {
            let nav = format!("\"nav_{}\"", &card["card_".len()..]);
            assert!(
                CATALOG_SCENE.contains(&nav),
                "card {card} has its bookmark {nav}"
            );
        }

        // COVERAGE — derived from the ROSTER, the single source of truth: every
        // engine component kind must appear somewhere in the tray (its own card,
        // or inside a composite's demo, e.g. `tabs` as the PTT's page rail). A
        // newly promoted kind fails HERE, by name, until its demo is authored —
        // the catalog can never silently lag the engine again.
        fn kinds_in(n: &UiNode, out: &mut std::collections::HashSet<String>) {
            out.insert(n.component.clone());
            for c in &n.children {
                kinds_in(c, out);
            }
        }
        let mut present = std::collections::HashSet::new();
        kinds_in(&tree, &mut present);
        let missing: Vec<&str> = flicker::ui::rust_component_kinds()
            .iter()
            .copied()
            .filter(|k| !present.contains(*k))
            .collect();
        assert!(
            missing.is_empty(),
            "engine kinds with no demo in the catalog: {missing:?}"
        );
    }

    /// **PRESENCE GATE for the `surface` card.** `surface` is a STRUCTURAL kind (the one
    /// kind at two depths — the root screen and every nested live surface), so the
    /// roster-coverage gate above, which derives its demand from the Rust component
    /// roster, no longer reaches it. The live-scene container's exerciser (card_26: a
    /// quad-layout nested surface the `ViewportFiller` fills, orbit on left-drag) is
    /// ratified catalog content (BDE5BFD0 / A8C9F02B), so its absence is a failure
    /// here, by name — a drift gate must cover the channel the drift travels (8634C200).
    #[test]
    fn the_surface_kind_keeps_its_live_catalog_card() {
        let json: serde_json::Value =
            serde_json::from_str(CATALOG_SCENE).expect("the catalog scene is JSON");
        fn find<'a>(v: &'a serde_json::Value, id: &str) -> Option<&'a serde_json::Value> {
            match v {
                serde_json::Value::Object(m) => {
                    if m.get("id").and_then(|i| i.as_str()) == Some(id) {
                        return Some(v);
                    }
                    m.values().find_map(|c| find(c, id))
                }
                serde_json::Value::Array(a) => a.iter().find_map(|c| find(c, id)),
                _ => None,
            }
        }
        let node =
            find(&json, "cat_surface").expect("the catalog carries the `cat_surface` card node");
        assert_eq!(
            node.get("component").and_then(|c| c.as_str()),
            Some("surface"),
            "the card's node is a `surface`"
        );
        assert!(
            node.get("layout").and_then(|l| l.as_str()).is_some(),
            "the card authors a `layout` so the filler tiles more than one pane"
        );
        assert!(
            CATALOG_SCENE.contains("\"nav_26\"") && CATALOG_SCENE.contains("\"card_26\""),
            "the surface card keeps its bookmark and its card box"
        );
    }

    // ───────────────────────────────────────────────────────────────
    // The three PAGES that landed 2026-09-03 (B05B3D09 §4b/§4c/§4d)
    // ───────────────────────────────────────────────────────────────

    /// **The PINNED FILLER ROSTER.** Unlike the kind cards — which derive their demand
    /// from `rust_component_kinds()` — a surface filler lives in another crate and no
    /// engine roster names it, so the demand has to be written down somewhere. Here:
    /// each entry is the filler and the `surface` node id its card seats it on. A
    /// filler landing without a card fails HERE, by name (rule 8634C200: a drift gate
    /// must cover the channel the drift travels).
    const FILLER_ROSTER: [(&str, &str); 7] = [
        ("Plot/sparkline", PLOT_SPARK),
        ("Plot/bars", PLOT_BARS),
        ("Plot/curve", PLOT_CURVE),
        ("GraphCanvas", GRAPH_SLOT),
        ("Timeline", TIMELINE_SLOT),
        ("Gadget", GADGET_SLOT),
        ("Doll", DOLL_SLOT),
    ];

    /// The RECIPES page's card ids, in tray order — the arrangements the catalog
    /// documents as the registry of record (A466E4C7). Every one of them must be a
    /// pure arrangement: roster kinds, shared style blocks, and no filler.
    const RECIPE_CARDS: [&str; 11] = [
        "card_29", "card_30", "card_31", "card_32", "card_33", "card_34", "card_35", "card_36",
        "card_37", "card_38", "card_39",
    ];

    /// The bench, built from the SHIPPED scene file exactly as the manifest builds it.
    fn bench() -> ComponentCatalog {
        let def = SceneDef::parse("componentcatalog", CATALOG_SCENE)
            .expect("componentcatalog.scene.json loads");
        ComponentCatalog::new(&def)
    }

    /// One headless walk of the REAL tray at 1600×900, through the bench's own model
    /// and its own `rows_from` expansion — the same two steps `update` takes, so what
    /// this measures is what the window lays out.
    fn walk(cat: &ComponentCatalog) -> flicker::ui::UiFrame {
        let mut model = cat.model();
        let tree = instantiate_rows(
            cat.tree.as_ref().expect("the scene ships a tree"),
            &mut model,
            &|source| cat.rows(source),
        );
        let snap = UiInput {
            mouse: flicker::render::Vec2::new(-1.0, -1.0),
            clicked: false,
            down: false,
            right_down: false,
            screen: flicker::render::Vec2::new(1600.0, 900.0),
            wheel: 0.0,
            exclusive: false,
            motion: Default::default(),
        };
        run_ui(
            &tree,
            &model,
            &cat.ui_styles,
            &snap,
            &mut UiState::default(),
        )
    }

    /// **EVERY PINNED FILLER HAS A LIVE CARD** — its `surface` node is authored in the
    /// tray, that node is inside a `card_<i>` box with its bookmark, and the walk
    /// RESERVES it with real extent (a seat of zero pixels is a filler that draws
    /// nothing, which is the failure mode the extent rules exist for, 93B5000F).
    #[test]
    fn every_filler_in_the_roster_has_a_seated_card() {
        let json: serde_json::Value =
            serde_json::from_str(CATALOG_SCENE).expect("the catalog scene is JSON");
        // Which card box holds the node with this id?
        fn card_of(node: &serde_json::Value, card: Option<String>, want: &str) -> Option<String> {
            let id = node.get("id").and_then(|i| i.as_str()).unwrap_or("");
            let card = if id.starts_with("card_") {
                Some(id.to_string())
            } else {
                card
            };
            if id == want {
                return card;
            }
            node.get("children")
                .and_then(|c| c.as_array())
                .into_iter()
                .flatten()
                .find_map(|kid| card_of(kid, card.clone(), want))
        }

        let cat = bench();
        let frame = walk(&cat);
        let mut missing = Vec::new();
        for (filler, slot) in FILLER_ROSTER {
            let Some(card) = card_of(&json["tree"], None, slot) else {
                missing.push(format!("{filler}: no `{slot}` surface node in the tray"));
                continue;
            };
            let nav = format!("\"nav_{}\"", &card["card_".len()..]);
            if !CATALOG_SCENE.contains(&nav) {
                missing.push(format!("{filler}: card {card} has no bookmark {nav}"));
            }
            match frame.surface(slot) {
                Some(s) if s.w > 0.0 && s.h > 0.0 => {}
                Some(s) => missing.push(format!(
                    "{filler}: `{slot}` reserved {}x{} — a filler seated on nothing",
                    s.w, s.h
                )),
                None => missing.push(format!("{filler}: `{slot}` reserved NO slot")),
            }
        }
        assert!(
            missing.is_empty(),
            "every filler in the pinned roster needs a live card:\n{}",
            missing.join("\n")
        );

        // …and the two fillers that composite an OFFSCREEN PASS are unseated while
        // their card is scrolled out of the tray. A reserved rect out in the margin is
        // exactly where a composite would paint over the nav rail, so this guard is
        // the difference between a filler card and a bug that only shows up scrolled.
        let tray = frame.rect(CONTENT_SCROLL_BIND);
        for slot in [GADGET_SLOT, DOLL_SLOT] {
            assert!(
                frame.surface(slot).is_some(),
                "`{slot}` is reserved by the walker even below the fold"
            );
            assert!(
                seat_in_tray(frame.surface(slot), tray).is_none(),
                "`{slot}` sits far below the tray's viewport at rest — it must NOT seat"
            );
        }
        // The same predicate seats it the moment the card is inside the viewport.
        let seated = frame.surface(PLOT_SPARK).map(|s| SurfaceSlot {
            y: tray.expect("the tray has a rect").pos.y,
            ..s.clone()
        });
        assert!(
            seat_in_tray(seated.as_ref(), tray).is_some(),
            "a card at the top of the tray seats"
        );
    }

    /// **NO BUTTON ON THIS PAGE CAN OPEN A TRAP.** Every demo params set this page
    /// hands the seam has a defined way OUT, and the seam reports it.
    ///
    /// Aaron was locked inside a modal opened from this very page and had to force-quit
    /// (B89FAC21). Two halves fix it: the shell always injects the exit (gated there,
    /// over the walker's own Cancel), and this page only offers trees the seam will
    /// host. This gate is the third: for each button's OWN demo params, the answer the
    /// player would get on Cancel is a real, non-empty name — the caller's action where
    /// it declared one, the host's `cancelled` where it did not (the `busy` demo, which
    /// deliberately declares none).
    ///
    /// WHEN that answer is reachable is the slab's own `dismissable` toggle (DA0E1B57):
    /// the busy demo holds Cancel while its job runs and lets go at the top of the bar.
    /// That is a delay, not a trap — which is why this gate asks what the exit SAYS and
    /// the shell's own gate asks when it opens.
    #[test]
    fn every_demo_params_set_has_a_way_out() {
        let cat = bench();
        for (action, id) in MODAL_BUTTONS {
            let params = cat.modal_params(id);
            let out = params.cancel_result();
            assert!(
                !out.is_empty(),
                "the `{action}` button opens `{id}` with no way out — a modal the \
                 player cannot leave is the trap of incident B89FAC21"
            );
            // Where this page named its own cancel verb, that verb is what comes back,
            // so `modal_closed` can tell a back-out from an answer.
            if id != &"busy" {
                assert_eq!(
                    out, "cat_modal_cancelled",
                    "`{id}`'s demo cancel reports the page's own verb"
                );
            }
        }
    }

    /// **THE POPUP_PANEL CARD DEMONSTRATES THE DISMISSABLE TOGGLE** (ruling DA0E1B57).
    ///
    /// The card is the catalog's job on this feature: a `dismissable_bind` on the slab,
    /// a `checkbox` writing the SAME key, and a pair script that seeds it ON. This gate
    /// pins all three to each other and then drives the engine's own reader
    /// ([`flicker::ui::popup_dismissable`]) through the real tray model — so the card
    /// cannot become a checkbox wired to nothing while claiming to demo the toggle.
    ///
    /// It also carries the FAIL-LOUD half: every `dismissable_bind` this scene authors
    /// must name a key the scene actually publishes, like any other bind. An unpublished
    /// one is not a trap (the reader falls back to the default, `dismissable`) — it is a
    /// FEATURE that silently does nothing, which is the class of defect the authored-
    /// style-path gate exists for.
    #[test]
    fn the_popup_panel_card_demonstrates_the_dismissable_toggle() {
        let cat = bench();
        let tree = cat.tree.as_ref().expect("the scene ships a tree");
        let model = cat.model();

        // Every `dismissable_bind` in the tray names a published key, and something
        // WRITES it (a bind nobody can move is a demo of nothing).
        fn binds(n: &UiNode, out: &mut Vec<(String, String)>) {
            if let Some(flicker::script::Value::Text(k)) = n.props.get("dismissable_bind") {
                out.push((n.id.clone(), k.clone()));
            }
            for c in &n.children {
                binds(c, out);
            }
        }
        let mut authored = Vec::new();
        binds(tree, &mut authored);
        assert!(
            !authored.is_empty(),
            "the popup_panel card must author `dismissable_bind` — the toggle is what \
             this card is for"
        );
        let two_way = tree_binds(tree);
        for (id, key) in &authored {
            assert!(
                model.get(key).is_some(),
                "`{id}` binds dismissable to `{key}`, which this scene never publishes \
                 — the toggle would silently do nothing (seed it in componentcatalog.lua)"
            );
            assert!(
                two_way.contains(key),
                "no control writes `{key}` — the card demos a toggle nobody can flip"
            );
        }

        // Seeded ON: the component's own default, restated where a reader can see it.
        assert!(
            model.is_on("cat_popup_dismissable"),
            "the pair script seeds the card's toggle ON — a modal is dismissable unless \
             something deliberately holds it"
        );
        assert!(
            flicker::ui::popup_dismissable(tree, &model),
            "…so the engine reads the card's slab as dismissable at rest"
        );

        // …and OFF holds it shut, through the engine's reader, not a copy of the rule.
        let mut held = model.clone();
        held.set("cat_popup_dismissable", false);
        assert!(
            !flicker::ui::popup_dismissable(tree, &held),
            "unticking the card's checkbox must make the slab refuse Cancel"
        );
    }

    /// **EVERY PARAM-DRIVEN MODAL HAS A BUTTON** on the Modals page, and every button
    /// names one the seam will actually host. The catalog is the EXERCISER of the host
    /// seam (1F0F7347) — a tree nobody can open from here is a tree nobody tests — but
    /// the demand is the SHELL'S REGISTRY, not the folder: `pause` / `confirm` /
    /// `settings` ship in `scenes/shared/` and are hosted by their own scenes, and
    /// wiring a button to one of those is what trapped Aaron in-window (B89FAC21).
    ///
    /// So this gate reads [`flicker_shell::param_driven_modals`] (the registry of
    /// record) and, in the same breath, pins that the scene-hosted three are NOT
    /// openable through the seam.
    #[test]
    fn every_param_driven_modal_has_a_modals_page_button() {
        let hostable: std::collections::BTreeSet<&str> =
            flicker_shell::param_driven_modals().into_iter().collect();
        assert!(
            !hostable.is_empty(),
            "the shell registers param-driven modals"
        );

        let wired: std::collections::BTreeSet<&str> =
            MODAL_BUTTONS.iter().map(|(_, id)| *id).collect();
        let untested: Vec<&&str> = hostable.iter().filter(|id| !wired.contains(*id)).collect();
        assert!(
            untested.is_empty(),
            "param-driven modals with no Modals-page button: {untested:?}"
        );
        let ghosts: Vec<&&str> = wired.iter().filter(|id| !hostable.contains(*id)).collect();
        assert!(
            ghosts.is_empty(),
            "Modals-page buttons naming ids the seam does not host: {ghosts:?}"
        );

        // THE TRAP GATE: the scene-hosted trees are refused BY NAME, and this page
        // offers none of them. Each names the scene that does host it, so the refusal
        // is a signpost rather than a dead end.
        for id in ["pause", "confirm", "settings"] {
            let host = flicker_shell::modal_host_of(id).unwrap_or_else(|| {
                panic!("`{id}` must be refused by the param seam — it has its own host")
            });
            assert!(
                !host.is_empty(),
                "`{id}`'s refusal names the scene that hosts it"
            );
            assert!(
                !wired.contains(id),
                "the Modals page offers `{id}`, which the seam refuses — that button \
                 opens an overlay with no working control and no exit (B89FAC21)"
            );
        }

        // …and each button is really AUTHORED, with its own nav ordinal, so the pad
        // reaches it. A wired id with no button would open nothing.
        for (action, id) in MODAL_BUTTONS {
            assert!(
                CATALOG_SCENE.contains(&format!("\"{action}\"")),
                "the `{id}` modal has no `{action}` button in the tray"
            );
        }
    }

    /// **EVERY RECIPE CARD IS A PURE ARRANGEMENT**: it names only kinds the engine's
    /// public roster knows, resolves only style blocks the merged styles carry, and
    /// seats NO filler. That is what makes a recipe card an answer to "this looks like
    /// a missing component" (F1BFA408: decompose before promoting) rather than a
    /// preview of one.
    #[test]
    fn every_recipe_card_is_an_arrangement_of_roster_kinds() {
        let json: serde_json::Value =
            serde_json::from_str(CATALOG_SCENE).expect("the catalog scene is JSON");
        fn find<'a>(v: &'a serde_json::Value, id: &str) -> Option<&'a serde_json::Value> {
            match v {
                serde_json::Value::Object(m) => {
                    if m.get("id").and_then(|i| i.as_str()) == Some(id) {
                        return Some(v);
                    }
                    m.values().find_map(|c| find(c, id))
                }
                serde_json::Value::Array(a) => a.iter().find_map(|c| find(c, id)),
                _ => None,
            }
        }
        fn collect(node: &serde_json::Value, kinds: &mut Vec<String>, styles: &mut Vec<String>) {
            if let Some(k) = node.get("component").and_then(|c| c.as_str()) {
                kinds.push(k.to_string());
            }
            for prop in ["style", "style_off", "panel_style", "glyph_style"] {
                if let Some(p) = node.get(prop).and_then(|p| p.as_str()) {
                    styles.push(p.to_string());
                }
            }
            if let Some(kids) = node.get("children").and_then(|c| c.as_array()) {
                for kid in kids {
                    collect(kid, kinds, styles);
                }
            }
        }

        let def = SceneDef::parse("componentcatalog", CATALOG_SCENE).expect("scene parses");
        let merged = flicker::ui::load_shared_styles(def.styles.as_ref());
        let mut broken = Vec::new();
        for card in RECIPE_CARDS {
            let node = find(&json["tree"], card)
                .unwrap_or_else(|| panic!("the recipes page carries {card}"));
            let (mut kinds, mut styles) = (Vec::new(), Vec::new());
            collect(node, &mut kinds, &mut styles);
            for k in &kinds {
                if !flicker::ui::is_known_kind(k) {
                    broken.push(format!("{card}: names unknown kind `{k}`"));
                }
                // A recipe documents CHROME. The moment one seats a live surface it
                // has stopped being an arrangement and belongs on the Fillers page.
                if k == "surface" {
                    broken.push(format!("{card}: seats a `surface` — that is a filler card"));
                }
            }
            for path in &styles {
                let resolved = path.split('.').try_fold(&merged, |v, seg| v.get(seg));
                if !resolved.is_some_and(serde_json::Value::is_object) {
                    broken.push(format!("{card}: style `{path}` resolves to no block"));
                }
            }
        }
        assert!(
            broken.is_empty(),
            "recipe cards must be pure arrangements:\n{}",
            broken.join("\n")
        );
    }

    /// **THE EXTENT GATE for the three new pages** (rule 93B5000F #5). A card can be
    /// perfectly formed in the tree and still resolve to zero pixels — invisible and
    /// unclickable — with every shape gate green. So: walk the REAL tray headless and
    /// assert presence AND size for every new card box and for every control a user
    /// must be able to see and hit, including the breadcrumb rows the bench expands.
    #[test]
    fn every_new_card_lays_out_with_real_extent() {
        let cat = bench();
        let frame = walk(&cat);
        let mut flat = Vec::new();
        let mut check = |id: &str| match frame.rect(id) {
            Some(r) if r.size.x > 0.0 && r.size.y > 0.0 => {}
            Some(r) => flat.push(format!("{id}: {}x{}", r.size.x, r.size.y)),
            None => flat.push(format!("{id}: NO rect")),
        };

        // Every card the three pages added, and every bookmark that scrolls to one.
        for i in 28..=47 {
            check(&format!("card_{i}"));
            check(&format!("nav_{i}"));
        }
        // The recipes' own controls — the half a user actually touches.
        for id in [
            "cat_rec_toast",
            "cat_rec_undo",
            "cat_rec_field",
            "cat_rec_select",
            "cat_rec_stepper",
            "cat_rec_tree_r1",
            "cat_rec_tree_r1_caret",
            "cat_rec_tree_r1_name",
            "cat_rec_tree_r3_name",
            "cat_rec_crumbs",
            "cat_rec_crumb_0",
            "cat_rec_crumb_2",
            "cat_rec_notice",
            "cat_rec_notice_copy",
            "cat_rec_tp_prev",
            "cat_rec_tp_play",
            "cat_rec_tp_next",
            "cat_rec_tp_readout",
            "cat_rec_gauge",
            "cat_rec_res_readout",
            "cat_rec_cmd",
            "cat_rec_cmd_0",
            "cat_rec_cmd_3",
            "cat_rec_collapse_hdr",
            "cat_rec_collapse_body",
            "cat_rec_drag_tile",
            "cat_rec_drop_target",
            "cat_rec_drop_readout",
            "cat_modal_grid",
            "cat_modal_result_val",
            "cat_modal_payload_val",
            // The popup_panel card's dismissable switch (DA0E1B57) — a checkbox that
            // measures zero is a toggle nobody can reach (the align/zero-extent class).
            "cat_popup_dismissable",
        ] {
            check(id);
        }
        // …and every Modals-page button, which is what makes the seam reachable.
        for (action, _) in MODAL_BUTTONS {
            check(action);
        }
        // The two GRID recipes really lay out in COLUMNS. A `grid` whose track spec
        // the walker could not read falls back to ONE column and stacks — which looks
        // like a styling quirk and is actually a dead prop name.
        let x_of = |id: &str| frame.rect(id).map(|r| r.pos.x).unwrap_or_default();
        assert!(
            x_of("cat_rec_cmd_3") > x_of("cat_rec_cmd_0"),
            "the command card's slots flow across its grid, not down one column"
        );
        // The Modals grid is 3 × 2: the third button sits to the RIGHT of the first
        // (the track spec was read) and the fourth WRAPS below it (the row spec was
        // read too). A dead track name collapses both into one column.
        let y_of = |id: &str| frame.rect(id).map(|r| r.pos.y).unwrap_or_default();
        assert!(
            x_of("cat_modal_prompt") > x_of("cat_modal_choice"),
            "the Modals page's buttons flow across their grid"
        );
        assert!(
            y_of("cat_modal_busy") > y_of("cat_modal_choice")
                && x_of("cat_modal_busy") <= x_of("cat_modal_choice"),
            "the Modals page's fourth button wraps onto the second row"
        );
        assert!(
            flat.is_empty(),
            "controls that exist in the tree and resolve to nothing:\n{}",
            flat.join("\n")
        );
    }

    /// **THE KIND-CARD BANK IS STILL DERIVED.** The three new pages are AUTHORED data
    /// with gates of their own; they must not have reintroduced the hand-kept
    /// bookkeeping D4 deleted (DCA4DFB2). So: the card list still comes off the tree,
    /// every card still carries its bookmark and the ordinals still chain, and the
    /// tray still covers the whole public roster with room to spare.
    #[test]
    fn the_card_bank_is_still_derived_from_the_tree() {
        let cat = bench();
        let tree = cat.tree.as_ref().expect("the scene ships a tree");
        let cards = card_ids(tree);
        assert_eq!(
            cards.len(),
            CATALOG_SCENE.matches("\"id\": \"card_").count(),
            "the derived card list IS the authored tray — no count is kept by hand"
        );
        assert!(
            cards.len() > flicker::ui::rust_component_kinds().len(),
            "the tray outgrew the kind roster (recipes + fillers + modals are extra cards, \
             not replacements)"
        );
        for (i, card) in cards.iter().enumerate() {
            assert_eq!(
                card,
                &format!("card_{i}"),
                "card boxes chain in authoring order"
            );
            assert!(
                CATALOG_SCENE.contains(&format!("\"nav_{i}\"")),
                "{card} has its bookmark"
            );
        }
        // The nav highlight is a pure function of the derived count — the pair script
        // lights one bookmark per card with no list of its own.
        let m = cat.model();
        assert_eq!(m.number("card_count"), Some(cards.len() as f64));
        assert!(
            m.text(&format!("nav_sty_{}", cards.len() - 1)).is_some(),
            "the pair script styles the LAST derived bookmark"
        );
    }

    /// **THE FILLER CARDS DRAW.** Seating is not enough: a filler that is seated and
    /// emits nothing is the same blank card as one that was never seated. Walk the
    /// tray, seat the three HUD-drawn fillers on the reserved rects and assert each
    /// one produced commands INSIDE the tray's scissor.
    #[test]
    fn the_hud_fillers_emit_clipped_commands_on_their_seats() {
        let mut cat = bench();
        let frame = walk(&cat);
        let tray = frame
            .rect(CONTENT_SCROLL_BIND)
            .expect("the tray has a rect");
        // The tray's rect must be its VIEWPORT, not the height of everything stacked
        // inside it — a scissor the size of the content would clip nothing at all.
        assert!(
            tray.size.y < 900.0,
            "the tray rect is the scrolling viewport ({}px), not the content run",
            tray.size.y
        );
        let graph = frame.surface(GRAPH_SLOT).cloned();
        let timeline = frame.surface(TIMELINE_SLOT).cloned();
        cat.plot_spark.seat(frame.surface(PLOT_SPARK));
        cat.plot_bars.seat(frame.surface(PLOT_BARS));
        cat.plot_curve.seat(frame.surface(PLOT_CURVE));
        assert!(
            cat.plot_spark.rect().is_some()
                && cat.plot_bars.rect().is_some()
                && cat.plot_curve.rect().is_some(),
            "all three plot readings take a seat"
        );

        cat.hud_commands.clear();
        cat.draw_fillers(
            Some(tray),
            false,
            graph.as_ref(),
            timeline.as_ref(),
            None,
            None,
        );
        let cmds = &cat.hud_commands;
        assert!(
            cmds.len() > 3,
            "the fillers drew {} commands — a seated filler that emits nothing is a \
             blank card with a green shape gate",
            cmds.len()
        );
        assert!(
            matches!(cmds.first(), Some(HudCommand::Clip { rect: Some(_) })),
            "the filler run opens with the tray's scissor"
        );
        assert!(
            matches!(cmds.last(), Some(HudCommand::Clip { rect: None })),
            "…and restores the full frame after it, so nothing downstream inherits it"
        );
        // Line primitives are what the Plot and the GraphCanvas both draw with
        // (F652B72F) — their presence is the proof the fillers really ran.
        assert!(
            cmds.iter().any(|c| matches!(c, HudCommand::Line { .. })),
            "the plot + graph fillers draw line primitives"
        );
    }

    /// **THE SHARED-MODAL SEAM, ROUND TRIP.** Every button on the Modals page arms its
    /// registered tree with demo params, and the answer comes back through
    /// `Scene::modal_closed` into the two readouts the page binds — the channel a
    /// bench uses, exercised by the bench that documents it.
    #[test]
    fn the_modals_page_opens_every_tree_and_reads_its_answer_back() {
        // `modal_params` is TOTAL over the roster — every registered tree has demo
        // params of its own, built on the click with no GPU in sight.
        let cat = bench();
        for (_, id) in MODAL_BUTTONS {
            let _ = cat.modal_params(id);
        }
        let mut cat = bench();
        assert_eq!(
            cat.model().text("cat_modal_result_val"),
            Some(""),
            "nothing has answered yet"
        );
        cat.modal_closed("choice_dialog", "cat_modal_ok", Some("42"));
        let m = cat.model();
        assert_eq!(
            m.text("cat_modal_result_val"),
            Some("choice_dialog · cat_modal_ok")
        );
        assert_eq!(m.text("cat_modal_payload_val"), Some("42"));
        // A payload-less answer reads as the authored dash, never as empty chrome.
        cat.modal_closed("conflict", "cat_modal_cancelled", None);
        assert_eq!(
            cat.model().text("cat_modal_payload_val"),
            Some(strings::resolve("$cat_modal_none").as_ref())
        );
    }

    /// **THE RECIPE PAGE'S LIVE VERBS.** The transport steps, the collapsible group
    /// flips and the drag→drop readout fills — each folded from the ONE activation
    /// channel a click, a pad Confirm and a drop all arrive on.
    #[test]
    fn the_recipe_verbs_fold_through_the_one_activation_channel() {
        let mut cat = bench();
        assert!(cat.model().is_on("cat_rec_collapse_open"));

        // The DROP: the walker publishes `drop_id` + `drop_target` beside the fired
        // `drop_action`, exactly as it does for a real release over a target.
        let mut r = ValueMap::new();
        r.set("cat_rec_dropped", true);
        r.set("drop_id", "ore");
        r.set("drop_target", "cat_rec_drop_target");
        cat.fold_recipe_verbs(&r);
        assert_eq!(
            cat.model().text("cat_rec_drop_readout"),
            Some("ore · cat_rec_drop_target"),
            "the drop readout names the payload and the target it landed on"
        );

        let mut r = ValueMap::new();
        r.set("cat_rec_collapse_hdr", true);
        cat.fold_recipe_verbs(&r);
        assert!(
            !cat.model().is_on("cat_rec_collapse_open"),
            "the header button closes the group"
        );

        let mut r = ValueMap::new();
        r.set("cat_rec_tp_next", true);
        cat.fold_recipe_verbs(&r);
        assert_eq!(cat.tp_frame, 12, "the transport steps the shared play-head");
        assert_eq!(
            cat.model().text("cat_rec_tp_readout"),
            Some("12 / 240"),
            "…and the readout is that number, not a second copy of it"
        );
    }
}
