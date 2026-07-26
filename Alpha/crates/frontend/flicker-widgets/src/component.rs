//! The **Rust component walker** — the engine half of the component-UI model.
//!
//! A screen declares a tree of [`UiNode`]s (via its Lua `tree()` builder, parsed
//! by `flicker-script`); this module OWNS the rest: it lays the tree out into
//! rects, hit-tests the pointer against it, and draws each node with its Rust
//! **template**. `HudCommand` is the templates' internal output (fed to the
//! existing [`render_hud`](crate::render_hud)); it no longer crosses the Lua
//! boundary. Interaction rides the same two-way name channels the immediate HUD
//! used: a node's `bind` ↔ a `Model` key (values), its `action` → an event name,
//! both returned in the [`UiFrame::results`] `ValueMap`. So an app swaps
//! `script.update`+`script.draw`+`render_hud` for [`run_ui`] + `render_hud` and
//! keeps applying the very same result keys.
//!
//! Templates read their colours/sizes from the resolved `ui_elements.json` by a
//! dotted `style` path (`"paperdoll.fit.slider"`) — so the palette stays in one
//! place (Prism `theme.tokens`) and a node carries only its truly-local data.
//!
//! This is a match-based template registry today (one arm per component kind);
//! the arms are the "component definitions" (ContentForge `ComponentEntry`s) and
//! new kinds are added here in one place.

use std::collections::HashSet;

use flicker_render::Vec2;
use flicker_script::{FontRole, HudCommand, TextAlign, UiAnchor, UiNode, Value, ValueMap};
use serde_json::Value as Json;

/// The per-frame interaction snapshot handed to [`run_ui`] — the same data the
/// legacy `ScriptHost::update` received, in one struct.
pub struct UiInput {
    /// Cursor position (pixels, top-left origin).
    pub mouse: Vec2,
    /// Left-button press *edge* this frame (a fresh click).
    pub clicked: bool,
    /// Left-button *held* state (for slider drags).
    pub down: bool,
    /// Screen size (the root layout rect).
    pub screen: Vec2,
    /// Text committed by the keyboard this frame — appended to a focused
    /// `text_field`'s bound string. Empty on non-typing frames and for scenes with
    /// no keyboard wiring yet.
    pub typed: String,
    /// Backspace *edge* this frame — pops one char from a focused `text_field`.
    pub backspace: bool,
}

/// What a drag-source node picked up — the payload a **scene-owned** canvas resolves
/// on release (e.g. `kind: "clip", id: "walk_forward"` dropped onto a state node).
/// The walker only carries the payload; it never decides what a drop means.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DragPayload {
    /// Category of the dragged thing, from the source node's `drag_kind` prop.
    pub kind: String,
    /// Identity of the dragged thing — the source node's `drag_id` prop, else its `id`.
    pub id: String,
}

/// Retained interaction state the caller holds across frames: the sliders capturing
/// the mouse mid-drag (keyed by node id/bind), plus the in-flight drag payload. A
/// slider drag keeps updating — and keeps claiming the mouse — until the button
/// releases, even if the cursor leaves the track.
#[derive(Default)]
pub struct UiState {
    dragging: HashSet<String>,
    drag: Option<DragPayload>,
    /// The id of the single currently-open `select` popup, or `None`. Set/cleared
    /// entirely within the select hit arm (a closed field's click opens it; while
    /// open, any click closes it) — `derive(Default)` starts it `None`.
    open: Option<String>,
    /// The id of the `text_field` that currently owns keyboard focus, or `None`.
    /// run_ui clears it at the top of any clicked frame; a click landing in a
    /// text_field re-establishes it in that field's hit arm — `Default` starts `None`.
    focus: Option<String>,
}

impl UiState {
    /// A fresh, empty interaction state.
    pub fn new() -> Self {
        Self::default()
    }

    /// The payload currently in flight, if any — so a scene-owned canvas can
    /// highlight valid drop targets mid-drag.
    pub fn drag(&self) -> Option<&DragPayload> {
        self.drag.as_ref()
    }

    /// Abandon an in-flight drag (e.g. the scene rejected the drop).
    pub fn cancel_drag(&mut self) {
        self.drag = None;
    }

    /// The id of the `text_field` that currently owns keyboard focus, if any.
    pub fn focused(&self) -> Option<&str> {
        self.focus.as_deref()
    }

    /// Programmatically give keyboard focus to a `text_field` by its node `id` —
    /// the scene's hook for focus-by-keypress (e.g. pressing **T** to enter chat),
    /// since focus is otherwise established only by a click landing in the field.
    /// `run_ui` clears focus at the top of any *clicked* frame, so a scene that
    /// wants a field to stay focused across clicks elsewhere re-asserts this each
    /// frame BEFORE `run_ui`.
    pub fn request_focus(&mut self, id: impl Into<String>) {
        self.focus = Some(id.into());
    }

    /// Drop keyboard focus (e.g. Escape leaves the chat input).
    pub fn clear_focus(&mut self) {
        self.focus = None;
    }
}

/// One `stage` node's reserved picture-in-picture slot — a rect the walker laid
/// out but deliberately does not fill.
///
/// The walker runs late (its commands are main-frame draws), while
/// `FrameGraph::execute` must run FIRST in a scene's `render()` — the offscreen
/// passes reset the shared per-frame draw queues. A `stage` node therefore
/// *reserves* its rect here, and the scene feeds the slot to its frame graph:
///
/// ```text
/// fg.target(handle, clear, |r| draw_source(r, source));
/// fg.composite_panel(handle, CompositeTarget::Screen, rect, layer, tint, None, None);
/// ```
///
/// Passing `frame: None` to that composite is deliberate: the walker has already
/// drawn the node's `panel` style as the backdrop through the normal 2D path, so
/// the graph only blits the image. That keeps every panel in the codebase drawn
/// by exactly one code path.
#[derive(Clone, Debug, PartialEq)]
pub struct StageSlot {
    /// The node's `id` — a scene keys its per-slot render target off this.
    pub id: String,
    /// Which `stages.<source>` sub-scene to render (the node's `source` prop).
    pub source: String,
    /// The IMAGE rect in screen pixels — already inset inside the node's frame.
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    /// Sub-layer, matching the node's own draw commands.
    pub layer: f32,
    /// Whether this slot should render a FRESH target this frame; `false` means the
    /// scene should blit its cached poster instead. N live targets cost N GPU
    /// submits per frame and a pack-browser screen carries ~14 stages, so liveness
    /// is authored data (`live` / `live_bind`), not a renderer detail.
    pub live: bool,
    /// Composite tint (default opaque white), from the node's `tint` dotted colour
    /// path or its style block.
    pub tint: [f32; 4],
}

/// The output of one [`run_ui`] pass: the draw commands (for
/// [`render_hud`](crate::render_hud)) and the result values / fired events (for
/// the engine to apply — identical in shape to the old `update` return).
pub struct UiFrame {
    /// Draw commands, in painter's order — feed straight to `render_hud`.
    pub commands: Vec<HudCommand>,
    /// Toggles / slider values / fired actions, plus `hud_hit` (pointer over any
    /// UI region — so the scene behind must not pick through).
    pub results: ValueMap,
    /// PiP slots reserved by `stage` nodes this frame — see [`StageSlot`]. Empty
    /// for a tree with no stages, so existing callers are unaffected.
    pub stages: Vec<StageSlot>,
}

/// A geometry rect (pixels).
#[derive(Clone, Copy, Debug)]
struct Rect {
    x: f32,
    y: f32,
    w: f32,
    h: f32,
}

impl Rect {
    fn contains(&self, p: Vec2) -> bool {
        p.x >= self.x && p.x <= self.x + self.w && p.y >= self.y && p.y <= self.y + self.h
    }
    fn inset(&self, p: f32) -> Rect {
        self.inset_xy(p, p)
    }
    /// Inset left/right by `px`, top/bottom by `py` — per-axis padding. Clamped so a
    /// pad wider than the rect yields a zero (never negative) inner extent, keeping
    /// layout stable when a bar's horizontal inset would otherwise exceed its width.
    fn inset_xy(&self, px: f32, py: f32) -> Rect {
        Rect {
            x: self.x + px,
            y: self.y + py,
            w: (self.w - 2.0 * px).max(0.0),
            h: (self.h - 2.0 * py).max(0.0),
        }
    }
}

/// Effective horizontal inset for a node: `pad_x` when set, else the uniform `pad`.
fn pad_x(n: &UiNode) -> f32 {
    n.pad_x.unwrap_or(n.pad)
}
/// Effective vertical inset for a node: `pad_y` when set, else the uniform `pad`.
fn pad_y(n: &UiNode) -> f32 {
    n.pad_y.unwrap_or(n.pad)
}

/// A laid-out node: its resolved rect, whether it is interactive this frame, and
/// its sub-layer (accumulated down the tree from each node's optional `layer`
/// prop), so a node's draw commands can be lifted above / dropped below its
/// siblings' — e.g. the menu's Muse sprite sitting BELOW the popup panel.
struct Placed<'a> {
    node: &'a UiNode,
    rect: Rect,
    enabled: bool,
    layer: f32,
    /// Scissor clip inherited from a `scroll` ancestor (px x,y,w,h), or `None`.
    /// Propagated in `resolve`; the draw pass emits a `HudCommand::Clip` when it
    /// changes between placed nodes (tree order keeps a scroll subtree contiguous).
    clip: Option<[f32; 4]>,
}

// ── Run ────────────────────────────────────────────────────────────────────

/// Lay out, hit-test, and draw a component `tree` for one frame. `model` is the
/// engine's published values (read by `bind`), `styles` the resolved
/// `ui_elements.json` (colours/sizes by dotted `style` path), `input` the
/// pointer snapshot, `state` the retained drag capture. Returns the draw
/// commands + the results `ValueMap`.
pub fn run_ui(
    tree: &UiNode,
    model: &ValueMap,
    styles: &Json,
    input: &UiInput,
    state: &mut UiState,
) -> UiFrame {
    let screen = Rect { x: 0.0, y: 0.0, w: input.screen.x, h: input.screen.y };
    let mut placed = Vec::new();
    resolve(tree, screen, model, 0.0, None, &mut placed);

    // Hit-test pass: fold events + value edits into `results`, drag into `state`.
    let mut results = ValueMap::new();
    let mut hud_hit = false;
    // Click-away de-focus: a fresh click clears text_field focus up front; a click
    // that lands in a text_field re-establishes it in that field's hit arm below.
    if input.clicked {
        state.focus = None;
    }
    for p in &placed {
        hit_node(p, model, input, state, styles, &mut results, &mut hud_hit);
    }
    if !state.dragging.is_empty() {
        hud_hit = true;
    }

    // Drag channel: publish the in-flight payload and the release edge so a scene-owned
    // canvas can resolve the drop against its own geometry. Deliberately does NOT force
    // `hud_hit` — the drop usually lands on the scene (a graph node), not on the UI, so
    // the scene must still be allowed to pick.
    if let Some(d) = state.drag.clone() {
        results.set("drag_kind", d.kind.as_str());
        results.set("drag_id", d.id.as_str());
        if input.down {
            results.set("drag_active", true);
        } else {
            results.set("drag_dropped", true);
            state.drag = None;
        }
    }

    results.set("hud_hit", hud_hit);

    // Draw pass: values reflect this frame's edits (results override model).
    let mut commands = Vec::new();
    let mut cur_clip: Option<[f32; 4]> = None;
    for p in &placed {
        // Toggle the scissor clip at each scroll boundary: a scroll node's
        // descendants are contiguous in tree order, so one `Clip` opens the run and
        // the next node with a different clip closes it. Emitted outside the node's
        // command range so the layer-offset below never touches it.
        if p.clip != cur_clip {
            commands.push(HudCommand::Clip { rect: p.clip });
            cur_clip = p.clip;
        }
        let start = commands.len();
        draw_node(p, model, &results, styles, input, state, &mut commands);
        // Lift this node's commands onto its sub-layer. Within one layer the 2D
        // pipelines draw ui-panels before sprites before text, so without this a
        // sprite (the Muse) would cover a same-layer panel (the popup); a higher
        // `layer` on the popup subtree keeps it on top.
        if p.layer != 0.0 {
            for c in &mut commands[start..] {
                offset_layer(c, p.layer);
            }
        }
    }
    // Restore the full frame after a trailing clipped run so nothing downstream inherits it.
    if cur_clip.is_some() {
        commands.push(HudCommand::Clip { rect: None });
    }

    // Stage pass: `stage` nodes reserve a PiP slot for the scene's frame graph to
    // fill (the walker cannot — see `StageSlot`). Their backdrop panel was already
    // drawn above by the normal styled-box path, so only the INSET image rect
    // travels here.
    let mut stages = Vec::new();
    for p in &placed {
        if p.node.component != "stage" {
            continue;
        }
        let Some(source) = ptext(p.node, "source") else {
            tracing::warn!("stage node {:?} has no `source` prop — slot skipped", p.node.id);
            continue;
        };
        let st = style_of(p.node, styles);
        // `inset` may ride as a node prop or sit in the shared panel style, so a
        // whole family of stages can share one inset without repeating it.
        let inset = pnum(p.node, "inset")
            .map(|n| n as f32)
            .unwrap_or_else(|| jnum(st, "inset", 0.0));
        let img = p.rect.inset(inset);
        // Liveness: an explicit Model bind wins, then a literal `live` prop, else
        // live (the single-stage case should just work).
        let live = match ptext(p.node, "live_bind") {
            Some(key) => eff_bool(&results, model, key),
            None if p.node.props.contains_key("live") => pbool(p.node, "live"),
            None => true,
        };
        let tint = match ptext(p.node, "tint") {
            Some(path) => json_color(jpath(styles, path), [1.0; 4]),
            None => first_color(st, &["tint"], [1.0; 4]),
        };
        stages.push(StageSlot {
            id: p.node.id.clone(),
            source: source.to_string(),
            x: img.x,
            y: img.y,
            w: img.w,
            h: img.h,
            layer: p.layer,
            live,
            tint,
        });
    }

    UiFrame { commands, results, stages }
}

// ── Layout ───────────────────────────────────────────────────────────────────

fn resolve<'a>(
    node: &'a UiNode,
    rect: Rect,
    model: &ValueMap,
    layer: f32,
    clip: Option<[f32; 4]>,
    out: &mut Vec<Placed<'a>>,
) {
    if !visible(node, model) {
        return;
    }
    // A node's optional `layer` prop accumulates down the tree, so a whole
    // subtree (a styled popup + its buttons + labels) can sit above a backdrop.
    let layer = layer + pnum(node, "layer").map(|n| n as f32).unwrap_or(0.0);
    out.push(Placed { node, rect, enabled: enabled(node, model), layer, clip });
    if node.children.is_empty() || matches!(node.component.as_str(), "tabs" | "pill_toggle" | "select" | "context_menu") {
        return;
    }
    let inner = rect.inset_xy(pad_x(node), pad_y(node));
    match node.component.as_str() {
        // A scroll region: children flow as a column shifted up by the bound offset,
        // and the whole subtree is clipped to the viewport (`inner`). Content taller
        // than the viewport scrolls; the offset is clamped here and in `hit_node`.
        "scroll" => {
            let content_h = scroll_content_h(node, model);
            let max = (content_h - inner.h).max(0.0);
            let offset = node
                .bind
                .as_deref()
                .and_then(|b| model.number(b))
                .unwrap_or(0.0)
                .clamp(0.0, max as f64) as f32;
            // Reserve a right gutter for the scrollbar so content lays out (and clips)
            // to the LEFT of it — otherwise a right-aligned control underlaps the bar and
            // its edge gets shaved by the viewport clip.
            let gutter = pnum(node, "gutter").map(|n| n as f32).unwrap_or(16.0);
            let view_w = (inner.w - gutter).max(0.0);
            let content = Rect { x: inner.x, y: inner.y - offset, w: view_w, h: content_h };
            let view = Some([inner.x, inner.y, view_w, inner.h]);
            flow(node, content, model, layer, view, out, false);
        }
        "row" => flow(node, inner, model, layer, clip, out, true),
        "column" | "panel" => flow(node, inner, model, layer, clip, out, false),
        // page / stack / anything else: overlay children, each placed by its own anchor.
        _ => {
            for c in &node.children {
                if !visible(c, model) {
                    continue;
                }
                let r = anchored(c, inner, model);
                resolve(c, r, model, layer, clip, out);
            }
        }
    }
}

/// Flow children along the main axis (row = x, column = y), filling the cross
/// axis. Fixed `size` children take their length; `grow` children share the rest
/// by weight. Ported from the exploratory `ui/layout.lua` resolver.
fn flow<'a>(
    node: &'a UiNode,
    area: Rect,
    model: &ValueMap,
    layer: f32,
    clip: Option<[f32; 4]>,
    out: &mut Vec<Placed<'a>>,
    horizontal: bool,
) {
    let kids: Vec<&UiNode> = node.children.iter().filter(|c| visible(c, model)).collect();
    let n = kids.len();
    let main = if horizontal { area.w } else { area.h };

    let mut fixed = 0.0;
    let mut grow_total = 0.0;
    for c in &kids {
        match c.grow {
            Some(g) => grow_total += g,
            None => fixed += child_main(c, model, horizontal),
        }
    }
    let free = main - fixed - node.gap * n.saturating_sub(1) as f32;

    let mut pos = if horizontal { area.x } else { area.y };
    for c in &kids {
        let len = match c.grow {
            Some(g) if grow_total > 0.0 => free * g / grow_total,
            Some(_) => 0.0,
            None => child_main(c, model, horizontal),
        };
        let r = if horizontal {
            Rect { x: pos, y: area.y, w: len, h: area.h }
        } else {
            Rect { x: area.x, y: pos, w: area.w, h: len }
        };
        resolve(c, r, model, layer, clip, out);
        pos += len + node.gap;
    }
}

/// A scroll region's intrinsic content height — its visible children stacked as a
/// column (pad + inter-child gaps + each child's main size). The basis for the max
/// scroll offset (`content_h - viewport_h`), used by `resolve`, `hit_node`, and the
/// scrollbar in `draw_node`.
fn scroll_content_h(node: &UiNode, model: &ValueMap) -> f32 {
    let kids: Vec<&UiNode> = node.children.iter().filter(|c| visible(c, model)).collect();
    let gaps = node.gap * kids.len().saturating_sub(1) as f32;
    pad_y(node) * 2.0 + gaps + kids.iter().map(|c| child_main(c, model, false)).sum::<f32>()
}

/// Place an absolutely-anchored node's box within `parent` (corner/edge + offset).
/// A `width_frac`/`height_frac` prop sizes the box as a fraction of the parent
/// rect — the flex-style constraint a full-screen backdrop or a viewport-tall Muse
/// needs, so the tree stays built-once and adapts to any window size at layout time.
/// An `aspect` (width÷height) prop instead LOCKS width to the resolved height, so an
/// image keeps its proportions (the square Muse) instead of stretching with the window.
fn anchored(node: &UiNode, parent: Rect, model: &ValueMap) -> Rect {
    let m = measure(node, model);
    let h = node
        .height
        .or_else(|| pnum(node, "height_frac").map(|f| parent.h * f as f32))
        .unwrap_or(m.y);
    let w = match pnum(node, "aspect") {
        Some(aspect) => h * aspect as f32,
        None => node
            .width
            .or_else(|| pnum(node, "width_frac").map(|f| parent.w * f as f32))
            .unwrap_or(m.x),
    };
    let a = node.anchor.unwrap_or(UiAnchor::TopLeft);
    let x = match a {
        UiAnchor::TopLeft | UiAnchor::Left | UiAnchor::BottomLeft => parent.x,
        UiAnchor::Top | UiAnchor::Center | UiAnchor::Bottom => parent.x + (parent.w - w) * 0.5,
        UiAnchor::TopRight | UiAnchor::Right | UiAnchor::BottomRight => parent.x + parent.w - w,
    } + node.offset[0];
    let y = match a {
        UiAnchor::TopLeft | UiAnchor::Top | UiAnchor::TopRight => parent.y,
        UiAnchor::Left | UiAnchor::Center | UiAnchor::Right => parent.y + (parent.h - h) * 0.5,
        UiAnchor::BottomLeft | UiAnchor::Bottom | UiAnchor::BottomRight => parent.y + parent.h - h,
    } + node.offset[1];
    Rect { x, y, w, h }
}

/// A node's intrinsic box — explicit `width`/`height` win; a container measures
/// from its (visible) children; a leaf falls back to `size` (its main-axis len).
fn measure(node: &UiNode, model: &ValueMap) -> Vec2 {
    let kids: Vec<&UiNode> = node.children.iter().filter(|c| visible(c, model)).collect();
    let gaps = node.gap * kids.len().saturating_sub(1) as f32;
    match node.component.as_str() {
        "row" => {
            let w = node.width.unwrap_or_else(|| {
                pad_x(node) * 2.0 + gaps + kids.iter().map(|c| child_main(c, model, true)).sum::<f32>()
            });
            let h = node.height.unwrap_or_else(|| {
                pad_y(node) * 2.0
                    + kids.iter().map(|c| child_cross(c, model, true)).fold(0.0, f32::max)
            });
            Vec2::new(w, h)
        }
        "column" | "panel" => {
            let h = node.height.unwrap_or_else(|| {
                pad_y(node) * 2.0
                    + gaps
                    + kids.iter().map(|c| child_main(c, model, false)).sum::<f32>()
            });
            let w = node.width.unwrap_or_else(|| {
                pad_x(node) * 2.0
                    + kids.iter().map(|c| child_cross(c, model, false)).fold(0.0, f32::max)
            });
            Vec2::new(w, h)
        }
        "stack" => {
            // Overlay container: hug the largest child (so a styled panel sizes to
            // its content column while corner decorations anchor to its edges),
            // unless an explicit width/height/size overrides.
            let cw = kids.iter().map(|c| measure(c, model).x).fold(0.0_f32, f32::max);
            let ch = kids.iter().map(|c| measure(c, model).y).fold(0.0_f32, f32::max);
            Vec2::new(node.width.or(node.size).unwrap_or(cw), node.height.or(node.size).unwrap_or(ch))
        }
        _ => Vec2::new(
            node.width.or(node.size).unwrap_or(0.0),
            node.height.or(node.size).unwrap_or(0.0),
        ),
    }
}

fn child_main(c: &UiNode, model: &ValueMap, horizontal: bool) -> f32 {
    if let Some(s) = c.size {
        return s;
    }
    let m = measure(c, model);
    if horizontal {
        m.x
    } else {
        m.y
    }
}

fn child_cross(c: &UiNode, model: &ValueMap, horizontal: bool) -> f32 {
    let m = measure(c, model);
    if horizontal {
        m.y
    } else {
        m.x
    }
}

fn visible(node: &UiNode, model: &ValueMap) -> bool {
    match &node.visible_bind {
        Some(k) => model.is_on(k),
        None => true,
    }
}

fn enabled(node: &UiNode, model: &ValueMap) -> bool {
    match &node.enabled_bind {
        Some(k) => model.is_on(k),
        None => true,
    }
}

// ── Hit-test ─────────────────────────────────────────────────────────────────

fn hit_node(
    p: &Placed,
    model: &ValueMap,
    input: &UiInput,
    state: &mut UiState,
    styles: &Json,
    results: &mut ValueMap,
    hud_hit: &mut bool,
) {
    let node = p.node;
    let r = p.rect;

    // Drag source — prop-driven so ANY row/cell/panel can be one (no new component
    // kind). Pressing inside a node carrying `drag_kind` picks up a payload; `run_ui`
    // reports it, and the scene-owned canvas decides what the drop means.
    if input.clicked && p.enabled && state.drag.is_none() && r.contains(input.mouse) {
        if let Some(kind) = ptext(node, "drag_kind") {
            let id = ptext(node, "drag_id").unwrap_or(node.id.as_str()).to_string();
            state.drag = Some(DragPayload { kind: kind.to_string(), id });
            *hud_hit = true;
        }
    }

    match node.component.as_str() {
        "checkbox" => {
            if let Some(bind) = &node.bind {
                let bx = checkbox_box(node, r);
                let mut val = eff_bool(results, model, bind);
                if bx.contains(input.mouse) {
                    *hud_hit = true;
                    if input.clicked && p.enabled {
                        val = !val;
                    }
                }
                // Two-way bind: ALWAYS report the current value, so an engine that
                // reads the key unconditionally stays in sync on non-click frames
                // too (mirrors the old immediate HUD, which set every state each frame).
                results.set(bind.clone(), val);
            }
        }
        "toggle" => {
            // A switch flips (and always reports) its `bind` bool, exactly like the
            // checkbox — the hit region is the pill, sized from its style block.
            if let Some(bind) = &node.bind {
                let pill = toggle_pill(r, style_of(node, styles));
                let mut val = eff_bool(results, model, bind);
                if pill.contains(input.mouse) {
                    *hud_hit = true;
                    if input.clicked && p.enabled {
                        val = !val;
                    }
                }
                results.set(bind.clone(), val);
            }
        }
        "radio" => {
            // One option of an exclusive group: every row shares the group's
            // `bind` (holds the selected id) and carries its own `value` (this
            // row's id). Selected iff the group key currently equals `value`.
            if let (Some(bind), Some(value)) = (&node.bind, ptext(node, "value")) {
                let bx = checkbox_box(node, r);
                if bx.contains(input.mouse) {
                    *hud_hit = true;
                    // Clicking selects THIS row. The click target stays live for
                    // every row — never gated on whether it is the current
                    // selection. A click sets the group key unconditionally, so it
                    // wins over any sibling's echo whatever the placement order
                    // (mirrors the slider `focus_group`).
                    if input.clicked && p.enabled {
                        results.set(bind.clone(), value.to_string());
                    }
                }
                // Otherwise echo the group's current selection, but only fill the
                // key if no sibling (or this row's own click) has set it yet — so
                // an engine reading the key stays in sync every frame, click or not.
                if results.get(bind).is_none() {
                    if let Some(cur) = model.text(bind) {
                        results.set(bind.clone(), cur.to_string());
                    }
                }
            }
        }
        "button" => {
            if r.contains(input.mouse) {
                *hud_hit = true;
                if input.clicked && p.enabled {
                    if let Some(action) = &node.action {
                        results.set(action.clone(), true);
                    }
                }
            }
        }
        "cell" => {
            let hovering = r.contains(input.mouse);
            if hovering {
                *hud_hit = true;
            }
            // A cell toggles (and always reports) its `bind` like a checkbox.
            if let Some(bind) = &node.bind {
                let mut val = eff_bool(results, model, bind);
                if hovering && input.clicked && p.enabled {
                    val = !val;
                }
                results.set(bind.clone(), val);
            }
        }
        "scroll" => {
            // Wheel over the region scrolls it: the `wheel` prop names a Model key the
            // scene publishes with the frame's wheel delta; the offset rides `bind`,
            // clamped to the content, and is reported every frame like any two-way bind.
            if r.contains(input.mouse) {
                *hud_hit = true;
                if let Some(bind) = &node.bind {
                    let inner = r.inset_xy(pad_x(node), pad_y(node));
                    let max = (scroll_content_h(node, model) - inner.h).max(0.0);
                    let cur = eff_num(results, model, bind).unwrap_or(0.0) as f32;
                    let wheel = ptext(node, "wheel")
                        .and_then(|k| eff_num(results, model, k))
                        .unwrap_or(0.0) as f32;
                    let speed = pnum(node, "scroll_speed").map(|n| n as f32).unwrap_or(46.0);
                    let next = (cur - wheel * speed).clamp(0.0, max);
                    results.set(bind.clone(), next as f64);
                }
            }
        }
        "slider" => {
            let id = slider_id(node);
            let (track, grab) = slider_rects(node, r);
            let hovering = r.contains(input.mouse);
            if hovering {
                *hud_hit = true;
                // Clicking the track GRABS it for dragging (the padded grab region) —
                // but only when enabled, so a disabled/unwired slider can't be dragged.
                if input.clicked && p.enabled && grab.contains(input.mouse) {
                    state.dragging.insert(id.clone());
                }
            }
            if !input.down {
                state.dragging.remove(&id);
            }
            // Focus (a shared `focus_group` key, e.g. "fit_focus"): a click anywhere in
            // the row grabs focus; otherwise echo the engine's persisted focus so it
            // survives the cursor leaving the track. A click grabbing focus always wins
            // over an echo, whatever the sibling order (a click sets unconditionally;
            // an echo only fills an absent key).
            if let (Some(fg), Some(bind)) = (focus_group(node), &node.bind) {
                if hovering && input.clicked {
                    results.set(fg.to_string(), bind.clone());
                } else if results.get(fg).is_none() {
                    if let Some(cur) = model.text(fg) {
                        results.set(fg.to_string(), cur.to_string());
                    }
                }
            }
            // Two-way value bind: a drag writes the mapped value; else report current.
            if let Some(bind) = &node.bind {
                let (min, max) = slider_range(node);
                let val = if state.dragging.contains(&id) {
                    *hud_hit = true;
                    let t = ((input.mouse.x - track.x) / track.w).clamp(0.0, 1.0);
                    (min + t * (max - min)) as f64
                } else {
                    eff_num(results, model, bind).unwrap_or(min as f64)
                };
                results.set(bind.clone(), val);
            }
        }
        "stepper" => {
            // A −[value]+ numeric box: clicking an end button steps the bound
            // number by `step`, clamped to [min, max]. The current value is
            // reported EVERY frame (like the checkbox/slider) so an engine that
            // reads the key unconditionally stays in sync on non-click frames.
            if r.contains(input.mouse) {
                *hud_hit = true;
            }
            if let Some(bind) = &node.bind {
                let (min, max) = slider_range(node);
                let (_field, minus, plus) = stepper_rects(node, r);
                let step = pnum(node, "step").unwrap_or(1.0);
                let mut val = eff_num(results, model, bind).unwrap_or(min as f64);
                if input.clicked && p.enabled {
                    if minus.contains(input.mouse) {
                        val = (val - step).clamp(min as f64, max as f64);
                    } else if plus.contains(input.mouse) {
                        val = (val + step).clamp(min as f64, max as f64);
                    }
                }
                results.set(bind.clone(), val);
            }
        }
        "pill_toggle" => {
            if let Some(bind) = &node.bind {
                let st = style_of(node, styles);
                let (well, segs) = pill_rects(node, st, r);
                if well.contains(input.mouse) {
                    *hud_hit = true;
                }
                // Start from the current selection (this frame's edit, else the model).
                let mut val = eff_text(results, model, bind).map(|s| s.to_string());
                if input.clicked && p.enabled {
                    for (seg, child) in segs.iter().zip(node.children.iter()) {
                        if seg.contains(input.mouse) {
                            if let Some(v) = ptext(child, "value") {
                                val = Some(v.to_string());
                            }
                        }
                    }
                }
                // Two-way bind: ALWAYS report the current selection each frame (like
                // the checkbox/slider arms), so an engine reading the key stays in sync.
                if let Some(v) = val {
                    results.set(bind.clone(), v);
                }
            }
        }
        "tabs" => {
            if let Some(bind) = &node.bind {
                // The strip claims the pointer like a button, so a click on it (even
                // between cells) doesn't pick through to the scene behind.
                if r.contains(input.mouse) {
                    *hud_hit = true;
                }
                // Current selection, defaulting to the first tab when the bind is
                // still unset — a tab strip always has exactly one tab active.
                let mut selected = eff_text(results, model, bind)
                    .map(str::to_string)
                    .or_else(|| node.children.first().and_then(|c| ptext(c, "value")).map(str::to_string));
                let n = node.children.len();
                for (i, child) in node.children.iter().enumerate() {
                    let tr = tab_rect(node, r, i, n);
                    if tr.contains(input.mouse) {
                        *hud_hit = true;
                        if input.clicked && p.enabled {
                            if let Some(value) = ptext(child, "value") {
                                selected = Some(value.to_string());
                            }
                        }
                    }
                }
                // Two-way bind: ALWAYS report the current tab id, so an engine that
                // reads the key unconditionally stays in sync on non-click frames
                // (mirrors the checkbox / slider arms).
                if let Some(sel) = selected {
                    results.set(bind.clone(), sel);
                }
            }
        }
        "select" => {
            let id = select_id(node);
            let is_open = state.open.as_deref() == Some(id.as_str());
            let (_field_st, menu_st) = select_styles(node, styles);
            // The field claims the pointer; while open, so does the popup menu.
            if r.contains(input.mouse) {
                *hud_hit = true;
            }
            if is_open && select_menu_rect(node, r, menu_st).contains(input.mouse) {
                *hud_hit = true;
            }
            // A click on an option (only while open) resolves to that option's value.
            // While open, ANY click closes the menu (mirrors the settings dropdown):
            // an option click also selects; a click on the field or outside just closes.
            // A click on the closed field opens it (toggling to this node's id).
            let mut chosen: Option<String> = None;
            if input.clicked && p.enabled {
                if is_open {
                    chosen = select_option_at(node, r, menu_st, input.mouse);
                    state.open = None;
                } else if r.contains(input.mouse) {
                    state.open = Some(id.clone());
                }
            }
            // Two-way bind: write the pick, else ALWAYS report the current value —
            // like the checkbox/slider, so an engine reading the key stays in sync.
            if let Some(bind) = &node.bind {
                let val = chosen.or_else(|| eff_text(results, model, bind).map(str::to_string));
                if let Some(v) = val {
                    results.set(bind.clone(), v);
                }
            }
        }
        "text_field" => {
            if let Some(bind) = &node.bind {
                if r.contains(input.mouse) {
                    *hud_hit = true;
                    // A fresh click in the well takes focus. run_ui cleared focus at
                    // the top of a clicked frame, so a click elsewhere leaves it clear.
                    if input.clicked && p.enabled {
                        state.focus = Some(node.id.clone());
                    }
                }
                // While focused, fold this frame's typed chars + a backspace edge
                // into the bound string; then ALWAYS report the value (two-way bind),
                // mirroring the checkbox/slider arms so a reader stays in sync even on
                // non-edit frames.
                let focused = state.focus.as_deref() == Some(node.id.as_str());
                let mut text = eff_text(results, model, bind).unwrap_or("").to_string();
                if focused && p.enabled {
                    text.push_str(&input.typed);
                    if input.backspace {
                        text.pop();
                    }
                }
                results.set(bind.clone(), text);
            }
        }
        "badge" => {
            if badge_pill(r, style_of(node, styles)).contains(input.mouse) {
                *hud_hit = true;
            }
        }
        "context_menu" => {
            // The whole menu surface claims the pointer, so a click anywhere on it
            // (a gap, a divider, or a disabled row) never picks through to the scene
            // behind — exactly as the open select popup does.
            if r.contains(input.mouse) {
                *hud_hit = true;
            }
            if input.clicked && p.enabled {
                let row_h = jnum(style_of(node, styles), "row_h", 30.0);
                for (i, c) in node.children.iter().enumerate() {
                    // Dividers and disabled items are inert; only a live row fires.
                    if pbool(c, "divider") || pbool(c, "disabled") {
                        continue;
                    }
                    if context_menu_row(r, row_h, i).contains(input.mouse) {
                        if let Some(action) = &c.action {
                            results.set(action.clone(), true);
                        }
                    }
                }
            }
        }
        // A styled container (a panel) claims the pointer, so a click on the
        // panel background doesn't pick through to the scene. A `stage` claims it
        // too: the PiP image is UI surface, not a hole through to the world.
        "row" | "column" | "panel" | "stack" | "page" | "stage"
            if has_style(node, styles) && r.contains(input.mouse) =>
        {
            *hud_hit = true;
        }
        _ => {}
    }
}

// ── Draw ─────────────────────────────────────────────────────────────────────

fn draw_node(
    p: &Placed,
    model: &ValueMap,
    results: &ValueMap,
    styles: &Json,
    input: &UiInput,
    state: &UiState,
    out: &mut Vec<HudCommand>,
) {
    let node = p.node;
    let r = p.rect;
    // `style_bind` (a Model key holding a dotted style path) wins over a literal `style`, so a
    // node's fill/border can follow its state — the non-interactive pipeline tabs pick active vs
    // idle this way, one node per tab instead of a stack of visibility-toggled panels.
    let st = resolve_style(node, styles, model, results);
    match node.component.as_str() {
        // Styled boxes — including a `stage`, whose panel IS its PiP backdrop; the
        // scene's frame graph blits the render target over this (see `StageSlot`).
        "row" | "column" | "panel" | "stack" | "page" | "stage" => {
            if !st.is_null() {
                draw_panel_bg(r, st, out);
            }
        }
        "scroll" => {
            // The region's own backdrop (unclipped — its children carry the viewport
            // clip, its scrollbar must stay visible at the edge).
            if !st.is_null() {
                draw_panel_bg(r, st, out);
            }
            let inner = r.inset_xy(pad_x(node), pad_y(node));
            let content_h = scroll_content_h(node, model);
            let max = content_h - inner.h;
            if max > 0.0 {
                let offset = node
                    .bind
                    .as_deref()
                    .and_then(|b| eff_num(results, model, b))
                    .unwrap_or(0.0)
                    .clamp(0.0, max as f64) as f32;
                let bw = jnum(st, "bar_w", 4.0);
                let bx = inner.x + inner.w - bw;
                out.push(rect_cmd(Rect { x: bx, y: inner.y, w: bw, h: inner.h }, first_color(st, &["track"], STONE)));
                let thumb_h = (inner.h * (inner.h / content_h)).clamp(28.0_f32.min(inner.h), inner.h);
                let ty = inner.y + (offset / max) * (inner.h - thumb_h);
                out.push(rect_cmd(Rect { x: bx, y: ty, w: bw, h: thumb_h }, first_color(st, &["thumb"], SAP)));
            }
        }
        "text" => {
            let text = node_text(node, model, results);
            // Font size: an explicit `text_size` prop, else the node's layout height
            // (a single line is usually its own height), else a default.
            let size = pnum(node, "text_size").map(|n| n as f32).or(node.size).unwrap_or(14.0);
            // Colour: a dotted `color` path into a token-resolved rgba (text's escape
            // hatch, since colours can't ride as scalar props), else the style block.
            // `color_bind` names a Model key holding that same dotted path, so a row whose
            // STATE decides its colour (a conform provenance, a pass/fail check) rides the
            // one two-way name channel instead of needing a node per possible colour.
            let path = match ptext(node, "color_bind") {
                Some(key) => eff_text(results, model, key),
                None => ptext(node, "color"),
            };
            let color = match path {
                Some(p) => json_color(jpath(styles, p), INK),
                None => first_color(st, &["color", "label_color"], INK),
            };
            // Align WITHIN the node's box: centre/right resolve against the rect
            // width (the menu's title centres over the popup), left keeps the edge.
            let align = node_align(node);
            let x = match align {
                TextAlign::Center => r.x + r.w * 0.5,
                TextAlign::Right => r.x + r.w,
                TextAlign::Left => r.x,
            };
            push_text(out, x, r.y, &text, size, color, align, node_font(node), pbool(node, "italic"), pbool(node, "bold"), pnum(node, "tracking").map(|n| n as f32).unwrap_or(-1.0));
        }
        "button" => {
            let hovered = r.contains(input.mouse);
            draw_button(r, st, node, model, results, hovered, out);
        }
        "cell" => draw_cell(r, node, model, results, styles, out),
        "checkbox" => draw_checkbox(r, node, model, results, st, out),
        "toggle" => draw_toggle(r, node, model, results, st, out),
        "radio" => draw_radio(r, node, model, results, st, out),
        "slider" => draw_slider(r, node, model, results, st, out),
        "stepper" => draw_stepper(r, node, model, results, st, out),
        "pill_toggle" => draw_pill_toggle(r, node, model, results, st, out),
        "sprite" => draw_sprite(r, node, out),
        "tabs" => draw_tabs(r, node, model, results, st, styles, input, out),
        "select" => draw_select(r, node, model, results, styles, input, state, out),
        "text_field" => draw_text_field(r, node, model, results, st, state, input, out),
        "context_menu" => draw_context_menu(r, node, st, input, out),
        "badge" => draw_badge(r, node, model, results, st, out),
        "tooltip" => draw_tooltip(r, node, model, results, st, styles, out),
        "rune_corners" => draw_rune_corners(r, node, st, out),
        _ => {}
    }
}

/// An image node — blit a texture (its id in the `tex` prop, resolved by the
/// caller's `Textures` map like the immediate HUD's sprites) at the node's rect,
/// tinted white × `alpha`. The one non-vector menu element (the Muse); its size /
/// anchor / sub-layer come from the tree like any other node.
fn draw_sprite(r: Rect, node: &UiNode, out: &mut Vec<HudCommand>) {
    let Some(tex) = pnum(node, "tex") else {
        return;
    };
    let alpha = pnum(node, "alpha").map(|n| n as f32).unwrap_or(1.0);
    out.push(HudCommand::Sprite {
        tex: tex as u32,
        x: r.x,
        y: r.y,
        w: r.w,
        h: r.h,
        color: [1.0, 1.0, 1.0, alpha],
        layer: 0.0,
    });
}

/// Add a node's accumulated sub-layer onto one of its emitted commands (see the
/// draw loop in [`run_ui`]). Every `HudCommand` carries a `layer`.
fn offset_layer(c: &mut HudCommand, dl: f32) {
    match c {
        HudCommand::Rect { layer, .. }
        | HudCommand::Sprite { layer, .. }
        | HudCommand::Text { layer, .. }
        | HudCommand::Panel { layer, .. } => *layer += dl,
        // A clip toggle carries no layer — it rides submission order, not the sort.
        HudCommand::Clip { .. } => {}
    }
}

// ── Templates ────────────────────────────────────────────────────────────────

fn draw_panel_bg(r: Rect, st: &Json, out: &mut Vec<HudCommand>) {
    // Key-aliasing (same spirit as the button variants): a styled container reads
    // its fill from whichever of these its block carries — `fill_top/bot` (panels),
    // `bg_top/bot` (the menu's gradient backdrop), `overlay` (the pause/confirm dim),
    // or a single `color` (the bronze divider rule).
    let top = first_color(st, &["fill_top", "bg_top", "overlay", "panel_bg", "bg", "fill", "color"], PANEL);
    let bot = first_color(st, &["fill_bot", "bg_bot", "overlay", "panel_bg", "bg", "fill", "color"], top);
    let border = first_color(st, &["panel_border", "border"], [0.0; 4]);
    out.push(HudCommand::Panel {
        x: r.x,
        y: r.y,
        w: r.w,
        h: r.h,
        color: top,
        color2: bot,
        // `grad` direction from the style (0 flat · 1 vertical · 2 horizontal),
        // defaulting to vertical when the two stops differ — the horizontal scrim
        // fade over the Muse needs `grad: 2`.
        grad: jnum(st, "grad", if top == bot { 0.0 } else { 1.0 }),
        radius: jnum(st, "radius", 0.0),
        border: if border[3] > 0.0 { jnum(st, "border_w", 1.0) } else { 0.0 },
        border_color: border,
        // `feather` (default 0) lets a styled panel be a soft drop shadow — the
        // menu's popup shadow is just a feathered, offset panel behind the popup.
        feather: jnum(st, "feather", 0.0),
        layer: 0.0,
    });
}

fn draw_button(
    r: Rect,
    st: &Json,
    node: &UiNode,
    model: &ValueMap,
    results: &ValueMap,
    hovered: bool,
    out: &mut Vec<HudCommand>,
) {
    // Optional sapphire glow halo behind a primary button on hover.
    let glow = first_color(st, &["glow"], [0.0; 4]);
    if glow[3] > 0.0 && hovered {
        out.push(panel(
            Rect { x: r.x - 3.0, y: r.y - 3.0, w: r.w + 6.0, h: r.h + 6.0 },
            glow,
            glow,
            jnum(st, "radius", 3.0) + 3.0,
            0.0,
            [0.0; 4],
            4.0,
        ));
    }
    let top = if hovered {
        first_color(st, &["hover_top", "hot", "fill_top", "cell", "fill"], SAP)
    } else {
        first_color(st, &["fill_top", "cell", "fill"], SAP)
    };
    let bot = if hovered {
        first_color(st, &["hover_bot", "hot", "fill_bot", "cell", "fill"], top)
    } else {
        first_color(st, &["fill_bot", "cell", "fill"], top)
    };
    let border = if hovered {
        first_color(st, &["hover_border", "border"], [0.0; 4])
    } else {
        first_color(st, &["border"], [0.0; 4])
    };
    out.push(panel(
        r,
        top,
        bot,
        jnum(st, "radius", 3.0),
        if border[3] > 0.0 { jnum(st, "border_w", 1.0) } else { 0.0 },
        border,
        0.0,
    ));
    // The caption goes through the same resolver every other text-bearing node uses, so a
    // button's label can ride a `text_bind` (a Model-owned caption — an exclusive choice's
    // glyph, a commit button that reads COMMITTED once it has) and still fall back to a
    // literal `label`.
    let label = node_text(node, model, results);
    let lc = if hovered {
        first_color(st, &["hover_label", "label"], INK)
    } else {
        first_color(st, &["label"], INK)
    };
    let lsz = jnum(st, "label_size", pnum(node, "label_size").map(|n| n as f32).unwrap_or(14.0));
    push_text(out, r.x + r.w * 0.5, r.y + (r.h - lsz) * 0.5, &label, lsz, lc, TextAlign::Center, FontRole::Label, false, false, -1.0);
}

fn draw_cell(
    r: Rect,
    node: &UiNode,
    model: &ValueMap,
    results: &ValueMap,
    styles: &Json,
    out: &mut Vec<HudCommand>,
) {
    // Lit vs empty style by the enabled binding (a slot with no piece loaded).
    let loaded = enabled(node, model);
    let st = if loaded {
        jpath(styles, ptext(node, "style").unwrap_or(""))
    } else {
        jpath(styles, ptext(node, "style_off").unwrap_or(""))
    };
    let on = loaded && eff_bool(results, model, node.bind.as_deref().unwrap_or(""));
    let fill = if on {
        first_color(st, &["hot"], SAP)
    } else {
        first_color(st, &["cell"], PANEL)
    };
    out.push(panel(r, fill, fill, 0.0, 0.0, [0.0; 4], 0.0));
    let label = ptext(node, "label").unwrap_or_default();
    let lc = first_color(st, &["label"], INK);
    let lsz = jnum(st, "label_size", 12.0);
    push_text(out, r.x + r.w * 0.5, r.y + (r.h - lsz) * 0.5, label, lsz, lc, TextAlign::Center, FontRole::Label, false, false, -1.0);
}

fn draw_checkbox(
    r: Rect,
    node: &UiNode,
    model: &ValueMap,
    results: &ValueMap,
    st: &Json,
    out: &mut Vec<HudCommand>,
) {
    let bx = checkbox_box(node, r);
    let border = first_color(st, &["border"], [0.0; 4]);
    out.push(panel(
        bx,
        first_color(st, &["box"], PANEL),
        first_color(st, &["box"], PANEL),
        0.0,
        if border[3] > 0.0 { 1.0 } else { 0.0 },
        border,
        0.0,
    ));
    if eff_bool(results, model, node.bind.as_deref().unwrap_or("")) {
        let p = jnum(st, "pad", 3.0);
        out.push(panel(
            Rect { x: bx.x + p, y: bx.y + p, w: bx.w - 2.0 * p, h: bx.h - 2.0 * p },
            first_color(st, &["check"], INK),
            first_color(st, &["check"], INK),
            0.0,
            0.0,
            [0.0; 4],
            0.0,
        ));
    }
    // The row label sits to the right of the box.
    if let Some(label) = ptext(node, "label") {
        let lx = r.x + pnum(node, "label_x").map(|n| n as f32).unwrap_or(bx.w + 8.0);
        let lsz = pnum(node, "label_size").map(|n| n as f32).unwrap_or(13.0);
        push_text(out, lx, bx.y + (bx.h - lsz) * 0.5, label, lsz, INK, TextAlign::Left, FontRole::Body, false, false, -1.0);
    }
}

/// A radio button — one option of an exclusive group. Mirrors [`draw_checkbox`],
/// but the box and its inner mark are drawn round: on the SDF UI shader a corner
/// `radius` of half the (square) side clamps to a full circle. Selected when the
/// group's `bind` key equals this row's `value` prop; reuses a checkbox-style
/// block (`box`/`border`/`check`/`label`, e.g. `sim.checkbox_style`).
fn draw_radio(
    r: Rect,
    node: &UiNode,
    model: &ValueMap,
    results: &ValueMap,
    st: &Json,
    out: &mut Vec<HudCommand>,
) {
    let bx = checkbox_box(node, r);
    let border = first_color(st, &["border"], [0.0; 4]);
    out.push(panel(
        bx,
        first_color(st, &["box"], PANEL),
        first_color(st, &["box"], PANEL),
        bx.w * 0.5, // radius >= half-side -> the shader clamps it to a circle
        if border[3] > 0.0 { 1.0 } else { 0.0 },
        border,
        0.0,
    ));
    // Selected when this row's `value` matches the group's bound selection id.
    let selected = node
        .bind
        .as_deref()
        .zip(ptext(node, "value"))
        .map(|(bind, value)| eff_text(results, model, bind) == Some(value))
        .unwrap_or(false);
    if selected {
        let p = jnum(st, "pad", 3.0);
        let dot = Rect { x: bx.x + p, y: bx.y + p, w: bx.w - 2.0 * p, h: bx.h - 2.0 * p };
        out.push(panel(
            dot,
            first_color(st, &["check"], INK),
            first_color(st, &["check"], INK),
            dot.w * 0.5, // round filled dot
            0.0,
            [0.0; 4],
            0.0,
        ));
    }
    // The row label sits to the right of the circle (mirrors the checkbox).
    if let Some(label) = ptext(node, "label") {
        let lx = r.x + pnum(node, "label_x").map(|n| n as f32).unwrap_or(bx.w + 8.0);
        let lsz = pnum(node, "label_size").map(|n| n as f32).unwrap_or(13.0);
        let lc = first_color(st, &["label"], INK);
        push_text(out, lx, bx.y + (bx.h - lsz) * 0.5, label, lsz, lc, TextAlign::Left, FontRole::Body, false, false, -1.0);
    }
}

fn draw_slider(
    r: Rect,
    node: &UiNode,
    model: &ValueMap,
    results: &ValueMap,
    st: &Json,
    out: &mut Vec<HudCommand>,
) {
    let (track, _) = slider_rects(node, r);
    let (min, max) = slider_range(node);
    let value = eff_num(results, model, node.bind.as_deref().unwrap_or("")).unwrap_or(min as f64) as f32;
    let focused = focus_group(node)
        .zip(node.bind.as_deref())
        .map(|(fg, bind)| eff_text(results, model, fg) == Some(bind))
        .unwrap_or(false);

    // Row label (left column).
    let label_w = pnum(node, "label_w").map(|n| n as f32).unwrap_or(0.0);
    let value_w = pnum(node, "value_w").map(|n| n as f32).unwrap_or(0.0);
    let lsz = jnum(st, "label_size", 13.0);
    if let Some(label) = ptext(node, "label") {
        let lc = if focused { first_color(st, &["focus_label"], RUNE) } else { INK };
        push_text(out, r.x, r.y + (r.h - lsz) * 0.5, label, lsz, lc, TextAlign::Left, FontRole::Body, false, false, -1.0);
    }

    // Track + fill + handle.
    let track_col = if focused { first_color(st, &["focus_track"], STONE) } else { first_color(st, &["track"], STONE) };
    let fill_col = if focused { first_color(st, &["focus_fill"], RUNE) } else { first_color(st, &["fill"], SAP) };
    out.push(rect_cmd(track, track_col));
    let t = ((value - min) / (max - min)).clamp(0.0, 1.0);
    let fw = track.w * t;
    out.push(rect_cmd(Rect { x: track.x, y: track.y, w: fw, h: track.h }, fill_col));
    let fill_hi = first_color(st, &["fill_hi"], [0.0; 4]);
    if fill_hi[3] > 0.0 && fw > 0.0 {
        out.push(rect_cmd(Rect { x: track.x, y: track.y, w: fw, h: 1.0 }, fill_hi));
    }
    let hw = jnum(st, "handle_w", 9.0);
    out.push(rect_cmd(
        Rect { x: track.x + track.w * t - hw * 0.5, y: track.y - 4.0, w: hw, h: track.h + 8.0 },
        first_color(st, &["handle"], SAP),
    ));

    // Value readout (right column).
    if value_w > 0.0 {
        let vsz = jnum(st, "value_size", 12.0);
        let _ = label_w;
        push_text(out, r.x + r.w, r.y + (r.h - vsz) * 0.5, &fmt_val(value as f64, node), vsz, first_color(st, &["value_color"], DIM), TextAlign::Right, FontRole::Body, false, false, -1.0);
    }
}

/// A `−[value]+` numeric stepper: an optional left label, a field box with two
/// square end buttons, and the bound value centred between them. Mirrors
/// `widgets.lua`'s `stepper_draw` (and `draw_slider`'s value read + `fmt_val`).
fn draw_stepper(
    r: Rect,
    node: &UiNode,
    model: &ValueMap,
    results: &ValueMap,
    st: &Json,
    out: &mut Vec<HudCommand>,
) {
    let (field, minus, plus) = stepper_rects(node, r);
    let (min, _max) = slider_range(node);
    let value = eff_num(results, model, node.bind.as_deref().unwrap_or("")).unwrap_or(min as f64);

    let lsz = jnum(st, "label_size", 13.0);
    let label_col = first_color(st, &["label"], INK);

    // Optional row label in the reserved left column (mirrors the slider).
    if let Some(label) = ptext(node, "label") {
        push_text(out, r.x, r.y + (r.h - lsz) * 0.5, label, lsz, label_col, TextAlign::Left, FontRole::Body, false, false, -1.0);
    }

    // Field background, then the two square end buttons painted over it.
    out.push(rect_cmd(field, first_color(st, &["field", "box"], PANEL)));
    let btn_col = first_color(st, &["btn"], STONE);
    out.push(rect_cmd(minus, btn_col));
    out.push(rect_cmd(plus, btn_col));

    // Glyphs on the end buttons, the value centred in the field.
    push_text(out, minus.x + minus.w * 0.5, minus.y + (minus.h - lsz) * 0.5, "-", lsz, label_col, TextAlign::Center, FontRole::Label, false, false, -1.0);
    push_text(out, plus.x + plus.w * 0.5, plus.y + (plus.h - lsz) * 0.5, "+", lsz, label_col, TextAlign::Center, FontRole::Label, false, false, -1.0);
    let vsz = jnum(st, "value_size", lsz);
    push_text(out, field.x + field.w * 0.5, field.y + (field.h - vsz) * 0.5, &fmt_val(value, node), vsz, first_color(st, &["value_color", "label"], label_col), TextAlign::Center, FontRole::Body, false, false, -1.0);
}

/// A compact **segmented pill** (2–3 options, one active): a rounded well (track)
/// with the selected segment highlighted. Options ride as CHILD nodes — each a
/// `value`+`label` data node — and `bind` carries the selected `value`. Geometry &
/// colours mirror settings.lua's pill/segment draw via `settings.controls.pill`.
fn draw_pill_toggle(
    r: Rect,
    node: &UiNode,
    model: &ValueMap,
    results: &ValueMap,
    st: &Json,
    out: &mut Vec<HudCommand>,
) {
    let (well, segs) = pill_rects(node, st, r);
    let radius = jnum(st, "radius", 15.0);
    // Rounded well (the track): `bg` fill + a hairline `border`, the pill backdrop.
    let bg = first_color(st, &["bg"], PANEL);
    let border = first_color(st, &["border"], [0.0; 4]);
    out.push(panel(
        well,
        bg,
        bg,
        radius,
        if border[3] > 0.0 { jnum(st, "border_w", 1.0) } else { 0.0 },
        border,
        0.0,
    ));
    // The bound value drives which segment lights up.
    let selected = node.bind.as_deref().and_then(|b| eff_text(results, model, b));
    let lsz = jnum(st, "label_size", 11.0);
    for (seg, child) in segs.iter().zip(node.children.iter()) {
        let value = ptext(child, "value");
        let active = value.is_some() && value == selected;
        if active {
            // Active segment: an `active_top`→`active_bot` vertical gradient inset
            // 1px within the cell, radius one under the well's — the floating pill
            // highlight from settings.lua's segment draw.
            out.push(panel(
                Rect { x: seg.x + 1.0, y: seg.y, w: (seg.w - 2.0).max(0.0), h: seg.h },
                first_color(st, &["active_top"], SAP),
                first_color(st, &["active_bot"], SAP),
                (radius - 1.0).max(0.0),
                0.0,
                [0.0; 4],
                0.0,
            ));
        }
        let label = ptext(child, "label").unwrap_or_default();
        let lc = if active {
            first_color(st, &["active_label"], INK)
        } else {
            first_color(st, &["label"], DIM)
        };
        push_text(out, seg.x + seg.w * 0.5, seg.y + (seg.h - lsz) * 0.5, label, lsz, lc, TextAlign::Center, FontRole::Label, false, false, -1.0);
    }
}

/// An interactive horizontal tab strip. The tabs are the node's CHILDREN (each a
/// data carrier with a `value` id + a `label`/`text`); `bind` is the selected tab
/// id. `st` is the optional strip-background block (the node's own `style` path,
/// e.g. `assetpipeline.tab_bar`); each cell then draws with the `tab_active` or
/// `tab_idle` style block (dotted paths in the node's props, e.g.
/// `loomforge.tab_active` / `loomforge.tab_idle`), chosen per tab by whether it is
/// the selected one. Selection is read from `results`/`model` — the hit pass runs
/// first, so a click this frame is already reflected here.
#[allow(clippy::too_many_arguments)]
fn draw_tabs(
    r: Rect,
    node: &UiNode,
    model: &ValueMap,
    results: &ValueMap,
    st: &Json,
    styles: &Json,
    input: &UiInput,
    out: &mut Vec<HudCommand>,
) {
    // The strip's background bar (behind every cell), when the node carries a style.
    if !st.is_null() {
        draw_panel_bg(r, st, out);
    }
    let active_st = jpath(styles, ptext(node, "tab_active").unwrap_or(""));
    let idle_st = jpath(styles, ptext(node, "tab_idle").unwrap_or(""));
    let selected = eff_text(results, model, node.bind.as_deref().unwrap_or(""));
    let n = node.children.len();
    for (i, child) in node.children.iter().enumerate() {
        let tr = tab_rect(node, r, i, n);
        let value = ptext(child, "value").unwrap_or_default();
        let active = !value.is_empty() && selected == Some(value);
        let cst = if active { active_st } else { idle_st };
        let hovered = tr.contains(input.mouse);
        draw_tab_cell(tr, cst, child, hovered, out);
    }
}

/// Draw one tab cell — mirrors [`draw_button`]'s fill / hover / label structure so
/// the same style-block shape (`fill_top`/`fill_bot`/`hover_*`/`border`/`label`)
/// works, with `active_top`/`active_bot`/`active_label` aliases for the design
/// system's existing segment/pill token naming. The cell's label comes from the
/// child node's `label` (or `text`).
fn draw_tab_cell(r: Rect, st: &Json, node: &UiNode, hovered: bool, out: &mut Vec<HudCommand>) {
    let top = if hovered {
        first_color(st, &["hover_top", "hot", "fill_top", "active_top", "fill"], PANEL)
    } else {
        first_color(st, &["fill_top", "active_top", "fill"], PANEL)
    };
    let bot = if hovered {
        first_color(st, &["hover_bot", "hot", "fill_bot", "active_bot", "fill"], top)
    } else {
        first_color(st, &["fill_bot", "active_bot", "fill"], top)
    };
    let border = if hovered {
        first_color(st, &["hover_border", "border"], [0.0; 4])
    } else {
        first_color(st, &["border"], [0.0; 4])
    };
    out.push(panel(
        r,
        top,
        bot,
        jnum(st, "radius", 3.0),
        if border[3] > 0.0 { jnum(st, "border_w", 1.0) } else { 0.0 },
        border,
        0.0,
    ));
    let label = ptext(node, "label").or_else(|| ptext(node, "text")).unwrap_or_default();
    let lc = if hovered {
        first_color(st, &["hover_label", "active_label", "label"], INK)
    } else {
        first_color(st, &["active_label", "label"], INK)
    };
    let lsz = jnum(st, "label_size", pnum(node, "label_size").map(|n| n as f32).unwrap_or(13.0));
    push_text(out, r.x + r.w * 0.5, r.y + (r.h - lsz) * 0.5, label, lsz, lc, TextAlign::Center, FontRole::Label, false, false, -1.0);
}

/// A dropdown TRIGGER + its popup option menu. Closed: a styled field showing the
/// selected option's label (or the dim `placeholder`) with a caret. Open (when
/// `state.open` names this node): the option rows drawn directly below the field,
/// lifted a whole sub-layer so they cover the sibling content beneath — the same
/// field/menu split the settings dropdown uses (`settings.controls.{field,menu}`).
#[allow(clippy::too_many_arguments)]
fn draw_select(
    r: Rect,
    node: &UiNode,
    model: &ValueMap,
    results: &ValueMap,
    styles: &Json,
    input: &UiInput,
    state: &UiState,
    out: &mut Vec<HudCommand>,
) {
    let (field, menu) = select_styles(node, styles);

    // ── field (always drawn) ──
    let ftop = first_color(field, &["top"], PANEL);
    let fbot = first_color(field, &["bot"], ftop);
    let fborder = first_color(field, &["border"], [0.0; 4]);
    out.push(panel(
        r,
        ftop,
        fbot,
        jnum(field, "radius", 3.0),
        if fborder[3] > 0.0 { 1.0 } else { 0.0 },
        fborder,
        0.0,
    ));
    let lsz = jnum(field, "label_size", 15.0);
    let (label, placeholder) = select_display(node, model, results);
    let lc = if placeholder {
        first_color(field, &["placeholder", "caret", "label"], DIM)
    } else {
        first_color(field, &["label"], INK)
    };
    push_text(out, r.x + 14.0, r.y + (r.h - lsz) * 0.5, &label, lsz, lc, TextAlign::Left, FontRole::Body, false, false, -1.0);
    // A small downward caret from stacked rows (avoids a glyph-font dependency),
    // matching settings.lua's `caret`.
    draw_caret(out, r.x + r.w - 16.0, r.y + r.h * 0.5, 9.0, first_color(field, &["caret", "label"], DIM));

    // ── popup menu (only while open) ──
    if state.open.as_deref() != Some(select_id(node).as_str()) {
        return;
    }
    let menu_start = out.len();
    let menu_rect = select_menu_rect(node, r, menu);
    let mtop = first_color(menu, &["top"], STONE);
    let mbot = first_color(menu, &["bot"], mtop);
    let mborder = first_color(menu, &["border"], [0.0; 4]);
    out.push(panel(
        menu_rect,
        mtop,
        mbot,
        jnum(menu, "radius", 3.0),
        if mborder[3] > 0.0 { 1.0 } else { 0.0 },
        mborder,
        0.0,
    ));
    let row_h = jnum(menu, "row_h", 30.0);
    let msz = jnum(menu, "label_size", 15.0);
    let cur = node.bind.as_deref().and_then(|b| eff_text(results, model, b));
    for (i, c) in node.children.iter().enumerate() {
        let ry = menu_rect.y + i as f32 * row_h;
        let selected = cur == Some(option_value(c));
        // Highlight the selected row, else the hovered row (the menu block carries
        // both `sel_bg` and `hover_bg`).
        if selected {
            out.push(rect_cmd(Rect { x: r.x + 4.0, y: ry, w: r.w - 8.0, h: row_h }, first_color(menu, &["sel_bg"], SAP)));
        } else if (Rect { x: r.x, y: ry, w: r.w, h: row_h }).contains(input.mouse) {
            out.push(rect_cmd(Rect { x: r.x + 4.0, y: ry, w: r.w - 8.0, h: row_h }, first_color(menu, &["hover_bg"], STONE)));
        }
        let rc = if selected { first_color(menu, &["sel_label", "label"], INK) } else { first_color(menu, &["label"], INK) };
        push_text(out, r.x + 14.0, ry + (row_h - msz) * 0.5, option_label(c), msz, rc, TextAlign::Left, FontRole::Body, false, false, -1.0);
    }
    // Lift the whole popup a sub-layer above the field + sibling content, exactly
    // as run_ui lifts a `layer`-tagged subtree (settings.lua draws it at `L + 1`).
    for cmd in &mut out[menu_start..] {
        offset_layer(cmd, 1.0);
    }
}

/// A downward caret built from four stacked 1px rows of decreasing width (widest
/// at top), so it needs no glyph font — the settings dropdown's caret.
fn draw_caret(out: &mut Vec<HudCommand>, cx: f32, cy: f32, s: f32, color: [f32; 4]) {
    for i in 0..4 {
        let w = s * (1.0 - i as f32 / 4.0);
        out.push(rect_cmd(Rect { x: cx - w * 0.5, y: cy - 1.0 + i as f32, w, h: 1.0 }, color));
    }
}

/// A single-line text input in a sunk-black well (DS `TextField`). Draws the well
/// (top/bot fill, rounded, bordered), then `eff_text(bind)` — or the `placeholder`
/// in dim when the value is empty — left-aligned and vertically centred. When this
/// field owns the focus (`state.focus == node.id`) the border becomes the rune-light
/// ring and a block caret sits at the END of the text. V1 has no glyph metrics in
/// the walker, so the caret x is an em-fraction estimate (`caret_adv`, default ½em)
/// and there is no mid-string caret.
#[allow(clippy::too_many_arguments)]
fn draw_text_field(
    r: Rect,
    node: &UiNode,
    model: &ValueMap,
    results: &ValueMap,
    st: &Json,
    state: &UiState,
    input: &UiInput,
    out: &mut Vec<HudCommand>,
) {
    let focused = state.focus.as_deref() == Some(node.id.as_str());
    let hovered = r.contains(input.mouse);

    // The sunk-black well. Border: the rune-light ring when focused, the bronze
    // hover edge on hover, else the resting edge.
    let top = first_color(st, &["top", "fill_top", "bg"], STONE);
    let bot = first_color(st, &["bot", "fill_bot", "bg"], top);
    let border = if focused {
        first_color(st, &["caret", "focus_border", "border"], RUNE)
    } else if hovered {
        first_color(st, &["hover_border", "border"], DIM)
    } else {
        first_color(st, &["border"], STONE)
    };
    let border_w = if focused { 2.0 } else { 1.0 };
    out.push(panel(
        r,
        top,
        bot,
        jnum(st, "radius", 3.0),
        if border[3] > 0.0 { border_w } else { 0.0 },
        border,
        0.0,
    ));

    // Value (or the placeholder when empty), left-aligned, vertically centred.
    let lsz = jnum(st, "label_size", pnum(node, "label_size").map(|n| n as f32).unwrap_or(14.0));
    let pad_x = pnum(node, "text_pad").map(|n| n as f32).unwrap_or(8.0);
    let value = eff_text(results, model, node.bind.as_deref().unwrap_or("")).unwrap_or("");
    let (shown, color) = if value.is_empty() {
        (ptext(node, "placeholder").unwrap_or(""), first_color(st, &["placeholder"], DIM))
    } else {
        (value, first_color(st, &["label", "color"], INK))
    };
    let tx = r.x + pad_x;
    let ty = r.y + (r.h - lsz) * 0.5;
    push_text(out, tx, ty, shown, lsz, color, TextAlign::Left, FontRole::Body, false, false, -1.0);

    // Block caret at the END of the text while focused. The walker has no glyph
    // metrics, so the x is estimated as `caret_adv` ems per char (default ½em).
    if focused {
        let adv = lsz * pnum(node, "caret_adv").map(|n| n as f32).unwrap_or(0.5);
        let caret_x = tx + value.chars().count() as f32 * adv;
        let cw = pnum(node, "caret_w").map(|n| n as f32).unwrap_or(2.0);
        out.push(rect_cmd(Rect { x: caret_x, y: ty, w: cw, h: lsz }, first_color(st, &["caret"], RUNE)));
    }
}

/// An on/off **switch** (DS `Toggle`) — a rounded pill with a sliding knob, a
/// two-way bool via `bind` (distinct from the boxy checkbox). Off draws a flat
/// `off_bg` pill (`off_border`) with the knob at the LEFT (`knob_off`); on draws
/// a vertical `on_top`→`on_bot` gradient pill (`on_border`) with the knob at the
/// RIGHT (`knob_on`). Pill size (`w`,`h`) + colours all come from the style block
/// (`settings.controls.toggle`); geometry mirrors `settings.lua`'s `draw_toggle`.
fn draw_toggle(
    r: Rect,
    node: &UiNode,
    model: &ValueMap,
    results: &ValueMap,
    st: &Json,
    out: &mut Vec<HudCommand>,
) {
    let pill = toggle_pill(r, st);
    let radius = pill.h * 0.5;
    // Knob is the pill inset 3px on every side (a circle), sitting at whichever end.
    let knob = (pill.h - 6.0).max(0.0);
    let kr = knob * 0.5;
    if eff_bool(results, model, node.bind.as_deref().unwrap_or("")) {
        let border = first_color(st, &["on_border"], [0.0; 4]);
        out.push(panel(
            pill,
            first_color(st, &["on_top"], SAP),
            first_color(st, &["on_bot"], SAP),
            radius,
            if border[3] > 0.0 { 1.0 } else { 0.0 },
            border,
            0.0,
        ));
        let kx = pill.x + pill.w - pill.h + 3.0;
        out.push(panel(
            Rect { x: kx, y: pill.y + 3.0, w: knob, h: knob },
            first_color(st, &["knob_on"], RUNE),
            first_color(st, &["knob_on"], RUNE),
            kr,
            0.0,
            [0.0; 4],
            0.0,
        ));
    } else {
        let border = first_color(st, &["off_border"], [0.0; 4]);
        out.push(panel(
            pill,
            first_color(st, &["off_bg"], STONE),
            first_color(st, &["off_bg"], STONE),
            radius,
            if border[3] > 0.0 { 1.0 } else { 0.0 },
            border,
            0.0,
        ));
        out.push(panel(
            Rect { x: pill.x + 3.0, y: pill.y + 3.0, w: knob, h: knob },
            first_color(st, &["knob_off"], DIM),
            first_color(st, &["knob_off"], DIM),
            kr,
            0.0,
            [0.0; 4],
            0.0,
        ));
    }
}

/// A **RuneCorners** decoration (DS `RuneCorners { topGlyphs:[l,r], bottomGlyphs:[l,r] }`):
/// four Elder-Futhark glyphs at the node rect's four corners — the TOP pair drawn in
/// rune-light (`top`), the BOTTOM pair in dim bronze (`bot`), each inset from its corner
/// by the style's `inset`. A pure OVERLAY: no hit, no bind. Reuses the `settings.runes`
/// block (`size`/`inset`/`top`/`bot`) — the exact inset geometry `settings.lua` uses for
/// its window corner runes — so the piece owns only its four glyph strings (`tl`/`tr`/
/// `bl`/`br`). The right pair is right-aligned so its right edge mirrors the left pair's
/// left edge (settings.lua's `r - ins`); the bottom pair's top is inset up by `inset +
/// size`, mirroring the top pair's box so all four sit fully inside the rect. An optional
/// `glyph_size` prop overrides the style's `size` (settings.runes.size = 5 is tuned for
/// the tiny inlay dots; corner glyphs usually want a larger face).
fn draw_rune_corners(r: Rect, node: &UiNode, st: &Json, out: &mut Vec<HudCommand>) {
    let inset = jnum(st, "inset", 8.0);
    let size = pnum(node, "glyph_size").map(|n| n as f32).unwrap_or_else(|| jnum(st, "size", 14.0));
    let glow = first_color(st, &["top"], RUNE);
    let bronze = first_color(st, &["bot"], DIM);
    let tl = ptext(node, "tl").unwrap_or("ᛞ");
    let tr = ptext(node, "tr").unwrap_or("ᛝ");
    let bl = ptext(node, "bl").unwrap_or("ᚨ");
    let br = ptext(node, "br").unwrap_or("ᛟ");
    // Top pair (rune-light glow), inset from the two top corners.
    push_text(out, r.x + inset, r.y + inset, tl, size, glow, TextAlign::Left, FontRole::Rune, false, false, -1.0);
    push_text(out, r.x + r.w - inset, r.y + inset, tr, size, glow, TextAlign::Right, FontRole::Rune, false, false, -1.0);
    // Bottom pair (dim bronze), inset up from the two bottom corners by `inset + size`
    // so the glyph box mirrors the top pair and stays fully inside the rect.
    let by = r.y + r.h - inset - size;
    push_text(out, r.x + inset, by, bl, size, bronze, TextAlign::Left, FontRole::Rune, false, false, -1.0);
    push_text(out, r.x + r.w - inset, by, br, size, bronze, TextAlign::Right, FontRole::Rune, false, false, -1.0);
}

/// A floating info card (DS `Tooltip`). The SCENE positions its rect and gates it
/// with `visible_bind`; the walker only paints it. Backdrop = a flat `bg` well +
/// hairline `border` (both read by [`draw_panel_bg`]); then an OPTIONAL element
/// **rune** glyph top-left (FontRole::Rune, coloured by the node's dotted
/// `rune_color` path — the same lit-glyph treatment as the corner-glow runes), the
/// **name** headline (Display / "headings & names"), and a dim **meta** line (Body,
/// `school · cast · cost`). Purely presentational: there is NO hit arm, so a
/// cursor-following tip never steals the pointer. Colours/sizes come from a
/// `tooltip` style block (`bg`/`border`/`radius`/`pad`/`name_*`/`meta_*`); `name`,
/// `rune`, and `meta` each accept a literal prop OR a `*_bind` Model key (bind-first,
/// exactly like `node_text`).
#[allow(clippy::too_many_arguments)]
fn draw_tooltip(
    r: Rect,
    node: &UiNode,
    model: &ValueMap,
    results: &ValueMap,
    st: &Json,
    styles: &Json,
    out: &mut Vec<HudCommand>,
) {
    // Backdrop through the one styled-box path (draw_panel_bg reads `bg`/`border`/`radius`).
    if !st.is_null() {
        draw_panel_bg(r, st, out);
    }
    let inner = r.inset(jnum(st, "pad", 12.0));
    let name_sz = jnum(st, "name_size", 16.0);
    let meta_sz = jnum(st, "meta_size", 12.0);
    let line_gap = jnum(st, "gap", 4.0);

    // Optional element rune badge, top-left, in the Rune face and the colour from
    // its dotted `rune_color` path (defaulting to the rune-light). The walker has no
    // glyph metrics, so the text column indents past it by an em-fraction estimate
    // (same approach as draw_text_field's caret advance).
    let mut indent = 0.0;
    if let Some(glyph) = tip_field(node, model, results, "rune", "rune_bind") {
        let rc = match ptext(node, "rune_color") {
            Some(path) => json_color(jpath(styles, path), RUNE),
            None => RUNE,
        };
        push_text(out, inner.x, inner.y, &glyph, name_sz, rc, TextAlign::Left, FontRole::Rune, false, false, -1.0);
        indent = name_sz * 1.3 + 4.0;
    }

    // Name headline, then the dim meta line one row below it (both in the text
    // column right of the rune badge).
    if let Some(name) = tip_field(node, model, results, "name", "name_bind") {
        let nc = first_color(st, &["name_color"], INK);
        push_text(out, inner.x + indent, inner.y, &name, name_sz, nc, TextAlign::Left, FontRole::Display, false, false, -1.0);
    }
    if let Some(meta) = tip_field(node, model, results, "meta", "meta_bind") {
        let mc = first_color(st, &["meta_color"], DIM);
        push_text(out, inner.x + indent, inner.y + name_sz + line_gap, &meta, meta_sz, mc, TextAlign::Left, FontRole::Body, false, false, -1.0);
    }
}

/// A small rounded **badge** chip (DS `Badge`) — a fully-rounded pill (radius ≈
/// half its height) filled by its `tone`, with a centred label. Presentational: it
/// carries no `bind`; it just claims the pointer so the scene can't pick through it.
/// `tone` (`accent` / `neutral` / `bronze`, default `neutral`) selects the
/// `<tone>_bg` fill + `<tone>_label` text colour from the style block; a truthy
/// `solid` prop OVERRIDES the tone with the filled-bronze `solid_bg`/`solid_label`
/// pair. Pill geometry & colour keys mirror the pills in `draw_toggle`/`draw_button`
/// (a `settings`-style token block, e.g. `"badge"`).
fn draw_badge(
    r: Rect,
    node: &UiNode,
    model: &ValueMap,
    results: &ValueMap,
    st: &Json,
    out: &mut Vec<HudCommand>,
) {
    let pill = badge_pill(r, st);
    // Fully-rounded by default (radius = half-height): the SDF UI shader clamps a
    // radius >= half the short side to a capsule, exactly like the round radio/toggle.
    let radius = jnum(st, "radius", pill.h * 0.5);
    // `solid` (filled bronze) wins over `tone`; otherwise the tone selects its pair.
    let (bg, label_col) = if pbool(node, "solid") {
        (first_color(st, &["solid_bg"], INK), first_color(st, &["solid_label"], STONE))
    } else {
        match ptext(node, "tone").unwrap_or("neutral") {
            "accent" => (first_color(st, &["accent_bg"], SAP), first_color(st, &["accent_label"], INK)),
            "bronze" => (first_color(st, &["bronze_bg"], STONE), first_color(st, &["bronze_label"], INK)),
            _ => (first_color(st, &["neutral_bg"], PANEL), first_color(st, &["neutral_label"], DIM)),
        }
    };
    // Filled chip; an optional hairline `border` (transparent by default → none),
    // mirroring draw_button's border handling so a block CAN add a crisp edge later.
    let border = first_color(st, &["border"], [0.0; 4]);
    out.push(panel(
        pill,
        bg,
        bg,
        radius,
        if border[3] > 0.0 { jnum(st, "border_w", 1.0) } else { 0.0 },
        border,
        0.0,
    ));
    // Label through the shared text resolver (so a literal `label`, a `text`, or a
    // Model-fed `text_bind` — e.g. a live count — all work), centred in the pill.
    let label = node_text(node, model, results);
    let lsz = jnum(st, "label_size", pnum(node, "label_size").map(|n| n as f32).unwrap_or(11.0));
    push_text(
        out,
        pill.x + pill.w * 0.5,
        pill.y + (pill.h - lsz) * 0.5,
        &label,
        lsz,
        label_col,
        TextAlign::Center,
        FontRole::Label,
        false,
        false,
        -1.0,
    );
}

/// A standalone floating **context menu** (DS `ContextMenu`) — the reusable form of
/// the popup that `select` draws inline. The node rect IS the menu; its items ride
/// as CHILD data nodes, each carrying a `label` (or `text`), an optional right-aligned
/// `hint` keybind, `active`/`disabled` bools, a `divider` flag (a hairline separator
/// in place of a row), and an `action` fired on click. Reuses the settings dropdown's
/// menu block (`settings.controls.menu`: top/bot/border/radius/row_h/label/sel_bg/
/// sel_label/hover_bg), so it matches the `select` popup exactly. To float it above
/// siblings, put a `layer` prop on the node — run_ui lifts the whole subtree, the same
/// reason `select` lifts its inline popup (there the popup shares the field's node, so
/// it must lift internally; a context_menu owns its node, so no internal lift is needed).
fn draw_context_menu(r: Rect, node: &UiNode, st: &Json, input: &UiInput, out: &mut Vec<HudCommand>) {
    // Menu backdrop: top→bot fill, rounded, optional hairline border (mirrors the
    // select popup's menu panel).
    let mtop = first_color(st, &["top"], STONE);
    let mbot = first_color(st, &["bot"], mtop);
    let mborder = first_color(st, &["border"], [0.0; 4]);
    out.push(panel(
        r,
        mtop,
        mbot,
        jnum(st, "radius", 3.0),
        if mborder[3] > 0.0 { 1.0 } else { 0.0 },
        mborder,
        0.0,
    ));

    let row_h = jnum(st, "row_h", 30.0);
    let msz = jnum(st, "label_size", 15.0);
    for (i, c) in node.children.iter().enumerate() {
        let row = context_menu_row(r, row_h, i);
        // A divider draws a centred hairline instead of a label row.
        if pbool(c, "divider") {
            let line = Rect {
                x: r.x + 8.0,
                y: row.y + (row.h * 0.5).floor(),
                w: (r.w - 16.0).max(0.0),
                h: 1.0,
            };
            out.push(rect_cmd(line, first_color(st, &["divider", "border"], DIM)));
            continue;
        }
        let disabled = pbool(c, "disabled");
        let active = pbool(c, "active");
        // Row wash (inset 4px like the select popup): an active row takes the
        // selection fill; a hovered *live* row takes the hover fill.
        if active {
            out.push(rect_cmd(
                Rect { x: r.x + 4.0, y: row.y, w: (r.w - 8.0).max(0.0), h: row_h },
                first_color(st, &["sel_bg"], SAP),
            ));
        } else if !disabled && row.contains(input.mouse) {
            out.push(rect_cmd(
                Rect { x: r.x + 4.0, y: row.y, w: (r.w - 8.0).max(0.0), h: row_h },
                first_color(st, &["hover_bg"], STONE),
            ));
        }
        // Label (left): disabled dims, active uses the selected-label colour.
        let lc = if disabled {
            first_color(st, &["disabled", "label"], DIM)
        } else if active {
            first_color(st, &["sel_label", "label"], INK)
        } else {
            first_color(st, &["label"], INK)
        };
        let label = ptext(c, "label").or_else(|| ptext(c, "text")).unwrap_or_default();
        push_text(out, r.x + 14.0, row.y + (row_h - msz) * 0.5, label, msz, lc, TextAlign::Left, FontRole::Body, false, false, -1.0);
        // Optional right-aligned keybind hint, always dim.
        if let Some(hint) = ptext(c, "hint") {
            push_text(out, r.x + r.w - 14.0, row.y + (row_h - msz) * 0.5, hint, msz, first_color(st, &["hint"], DIM), TextAlign::Right, FontRole::Body, false, false, -1.0);
        }
    }
}

// ── Geometry helpers ─────────────────────────────────────────────────────────

/// A pill-toggle's geometry: the rounded **well** (a style-`h`-tall track centred
/// in the node rect) plus one **cell** rect per option child — the inner strip
/// (well inset by the style `pad`) split into equal segments. Draw & hit share it
/// so they agree exactly, mirroring settings.lua's pill/segment draw.
fn pill_rects(node: &UiNode, st: &Json, r: Rect) -> (Rect, Vec<Rect>) {
    let pad = jnum(st, "pad", 3.0);
    let sh = jnum(st, "h", r.h);
    let h = if r.h > 0.0 { sh.min(r.h) } else { sh };
    let well = Rect { x: r.x, y: r.y + ((r.h - h) * 0.5).max(0.0), w: r.w, h };
    let n = node.children.len();
    let mut segs = Vec::with_capacity(n);
    if n > 0 {
        let cw = (well.w - pad * 2.0) / n as f32;
        let ch = (well.h - pad * 2.0).max(0.0);
        for i in 0..n {
            segs.push(Rect { x: well.x + pad + i as f32 * cw, y: well.y + pad, w: cw, h: ch });
        }
    }
    (well, segs)
}

fn checkbox_box(node: &UiNode, r: Rect) -> Rect {
    let b = pnum(node, "box").map(|n| n as f32).unwrap_or(14.0);
    Rect { x: r.x, y: r.y, w: b, h: b }
}

/// A toggle's pill rect — a `w`×`h` switch (from the style block, defaulting to
/// the `settings.controls.toggle` 50×25) at the node's left edge, vertically
/// centred like the slider's track so it reads right in a row of any height.
fn toggle_pill(r: Rect, st: &Json) -> Rect {
    let w = jnum(st, "w", 50.0);
    let h = jnum(st, "h", 25.0);
    Rect { x: r.x, y: r.y + (r.h - h) * 0.5, w, h }
}

/// A slider row's track rect (inset by the label/value columns, vertically
/// centred) and its padded grab rect (the track ±6px, so a press just above/below
/// the thin track still grabs — matching `widgets.lua`).
fn slider_rects(node: &UiNode, r: Rect) -> (Rect, Rect) {
    let label_w = pnum(node, "label_w").map(|n| n as f32).unwrap_or(0.0);
    let value_w = pnum(node, "value_w").map(|n| n as f32).unwrap_or(0.0);
    let sh = pnum(node, "slider_h").map(|n| n as f32).unwrap_or(r.h);
    let track = Rect {
        x: r.x + label_w,
        y: r.y + (r.h - sh) * 0.5,
        w: (r.w - label_w - value_w).max(0.0),
        h: sh,
    };
    let grab = Rect { x: track.x, y: track.y - 6.0, w: track.w, h: track.h + 12.0 };
    (track, grab)
}

/// A stepper row's field box (inset past the optional left `label_w` column and
/// vertically centred within an optional `field_h`) plus its two square end
/// buttons — `−` on the left, `+` on the right, each as wide as the field is
/// tall. One source of geometry shared by `draw_stepper` and its hit arm.
fn stepper_rects(node: &UiNode, r: Rect) -> (Rect, Rect, Rect) {
    let label_w = pnum(node, "label_w").map(|n| n as f32).unwrap_or(0.0);
    let fh = pnum(node, "field_h").map(|n| n as f32).unwrap_or(r.h);
    let field = Rect {
        x: r.x + label_w,
        y: r.y + (r.h - fh) * 0.5,
        w: (r.w - label_w).max(0.0),
        h: fh,
    };
    let bw = field.h;
    let minus = Rect { x: field.x, y: field.y, w: bw, h: field.h };
    let plus = Rect { x: field.x + field.w - bw, y: field.y, w: bw, h: field.h };
    (field, minus, plus)
}

/// The `i`-th of `n` tab cells within a strip rect: the strip inset by its `pad`,
/// then split evenly along x with the node's `gap` between cells. Shared by the
/// tabs draw and hit arms so their geometry always agrees.
fn tab_rect(node: &UiNode, r: Rect, i: usize, n: usize) -> Rect {
    let inner = r.inset_xy(pad_x(node), pad_y(node));
    let gap = node.gap;
    let total_gap = gap * n.saturating_sub(1) as f32;
    let tw = ((inner.w - total_gap) / n.max(1) as f32).max(0.0);
    Rect { x: inner.x + i as f32 * (tw + gap), y: inner.y, w: tw, h: inner.h }
}

fn slider_range(node: &UiNode) -> (f32, f32) {
    (
        pnum(node, "min").map(|n| n as f32).unwrap_or(0.0),
        pnum(node, "max").map(|n| n as f32).unwrap_or(1.0),
    )
}

fn slider_id(node: &UiNode) -> String {
    if !node.id.is_empty() {
        node.id.clone()
    } else {
        node.bind.clone().unwrap_or_default()
    }
}

fn focus_group(node: &UiNode) -> Option<&str> {
    ptext(node, "focus_group")
}

/// A select's stable id (for `state.open`): its `id`, else its `bind` — mirrors
/// `slider_id`, so a select declared with only a bind still opens/closes cleanly.
fn select_id(node: &UiNode) -> String {
    if !node.id.is_empty() {
        node.id.clone()
    } else {
        node.bind.clone().unwrap_or_default()
    }
}

/// Resolve a select's two style sub-blocks — the closed `field` and the open
/// `menu` — from its single dotted `style` path (e.g. `"settings.controls"`),
/// reusing the exact blocks the settings dropdown draws from.
fn select_styles<'a>(node: &UiNode, styles: &'a Json) -> (&'a Json, &'a Json) {
    let base = style_of(node, styles);
    (
        base.get("field").unwrap_or(&Json::Null),
        base.get("menu").unwrap_or(&Json::Null),
    )
}

/// The popup menu's outer rect: flush under the field (a 6px gap, matching the
/// settings dropdown), `row_h` per option child.
fn select_menu_rect(node: &UiNode, r: Rect, menu_st: &Json) -> Rect {
    let row_h = jnum(menu_st, "row_h", 30.0);
    let n = node.children.len();
    Rect { x: r.x, y: r.y + r.h + 6.0, w: r.w, h: row_h * n as f32 }
}

/// The field's display text + whether it is the (dim) placeholder. Prefers the
/// selected option's label (matched on `value`); falls back to the raw bound
/// value, then to the `placeholder`.
fn select_display(node: &UiNode, model: &ValueMap, results: &ValueMap) -> (String, bool) {
    let cur = node
        .bind
        .as_deref()
        .and_then(|b| eff_text(results, model, b))
        .filter(|s| !s.is_empty());
    match cur {
        Some(cur) => {
            let label = node
                .children
                .iter()
                .find(|c| option_value(c) == cur)
                .map(option_label)
                .unwrap_or(cur);
            (label.to_string(), false)
        }
        None => (ptext(node, "placeholder").unwrap_or("").to_string(), true),
    }
}

/// An option child's bound value (its `value` prop, else its `label`).
fn option_value(child: &UiNode) -> &str {
    ptext(child, "value").or_else(|| ptext(child, "label")).unwrap_or("")
}

/// An option child's display label (its `label` prop, else its `value`).
fn option_label(child: &UiNode) -> &str {
    ptext(child, "label").or_else(|| ptext(child, "value")).unwrap_or("")
}

/// The value of the option row under `p` (menu assumed open), or `None` when the
/// point is on the field / outside the menu.
fn select_option_at(node: &UiNode, r: Rect, menu_st: &Json, p: Vec2) -> Option<String> {
    let menu = select_menu_rect(node, r, menu_st);
    if !menu.contains(p) {
        return None;
    }
    let row_h = jnum(menu_st, "row_h", 30.0);
    for (i, c) in node.children.iter().enumerate() {
        let row = Rect { x: r.x, y: menu.y + i as f32 * row_h, w: r.w, h: row_h };
        if row.contains(p) {
            return Some(option_value(c).to_string());
        }
    }
    None
}

fn fmt_val(v: f64, node: &UiNode) -> String {
    let dec = pnum(node, "decimals").unwrap_or(2.0) as usize;
    let sign = if pbool(node, "plus") && v >= 0.0 { "+" } else { "" };
    let suffix = ptext(node, "suffix").unwrap_or("");
    format!("{sign}{v:.dec$}{suffix}")
}

/// A badge's pill rect — a style-`h`-tall capsule inset horizontally by the style
/// `pad`, vertically centred within the node rect (mirrors `toggle_pill` /
/// `pill_rects`). Both `pad` (0) and `h` (the node-rect height) default to a pill
/// that fills the node's box, so a badge sized by the layout "just works"; a shorter
/// `h` yields a slim chip floating in a taller row.
fn badge_pill(r: Rect, st: &Json) -> Rect {
    let pad = jnum(st, "pad", 0.0);
    let sh = jnum(st, "h", r.h);
    let h = if r.h > 0.0 { sh.min(r.h) } else { sh };
    Rect {
        x: r.x + pad,
        y: r.y + ((r.h - h) * 0.5).max(0.0),
        w: (r.w - pad * 2.0).max(0.0),
        h,
    }
}

/// The `i`-th item row of a context menu — a full-width, `row_h`-tall band stacked
/// from the top of the menu rect. Shared by the draw and hit arms so their row
/// geometry always agrees (mirrors the inline row math in the `select` popup).
fn context_menu_row(r: Rect, row_h: f32, i: usize) -> Rect {
    Rect { x: r.x, y: r.y + i as f32 * row_h, w: r.w, h: row_h }
}

// ── Value / style / command helpers ──────────────────────────────────────────

fn node_text(node: &UiNode, model: &ValueMap, results: &ValueMap) -> String {
    let prefix = ptext(node, "prefix").unwrap_or("");
    let body = match ptext(node, "text_bind") {
        Some(key) => eff_text(results, model, key).unwrap_or_default(),
        None => ptext(node, "text").or(ptext(node, "label")).unwrap_or_default(),
    };
    format!("{prefix}{body}")
}

/// One tooltip content field: the Model text under its `<key>_bind` (this frame's
/// edit, else the model), else the literal `<key>` prop. An empty/absent value is
/// `None`, so `rune` and `meta` stay optional. Mirrors `node_text`'s bind-first read.
fn tip_field(node: &UiNode, model: &ValueMap, results: &ValueMap, lit: &str, bind: &str) -> Option<String> {
    let v = match ptext(node, bind) {
        Some(key) => eff_text(results, model, key),
        None => ptext(node, lit),
    };
    v.filter(|s| !s.is_empty()).map(|s| s.to_string())
}

fn node_align(node: &UiNode) -> TextAlign {
    match ptext(node, "align") {
        Some("center") => TextAlign::Center,
        Some("right") => TextAlign::Right,
        _ => TextAlign::Left,
    }
}

fn node_font(node: &UiNode) -> FontRole {
    match ptext(node, "font") {
        Some("display") => FontRole::Display,
        Some("label") => FontRole::Label,
        Some("rune") => FontRole::Rune,
        _ => FontRole::Body,
    }
}

fn has_style(node: &UiNode, styles: &Json) -> bool {
    !style_of(node, styles).is_null()
}

fn style_of<'a>(node: &UiNode, styles: &'a Json) -> &'a Json {
    match ptext(node, "style") {
        Some(path) => jpath(styles, path),
        None => &Json::Null,
    }
}

/// Like [`style_of`], but a node may name a Model key in `style_bind` that HOLDS the dotted
/// style path — so a node's whole styling can follow its STATE (an active vs idle tab) through
/// the one two-way name channel, exactly as a text node's `color_bind` does for its colour. A
/// literal `style` is the fallback when no bind is set, or the bound key is absent this frame.
fn resolve_style<'a>(node: &UiNode, styles: &'a Json, model: &ValueMap, results: &ValueMap) -> &'a Json {
    if let Some(key) = ptext(node, "style_bind") {
        if let Some(path) = eff_text(results, model, key) {
            return jpath(styles, path);
        }
    }
    match ptext(node, "style") {
        Some(path) => jpath(styles, path),
        None => &Json::Null,
    }
}

/// Walk a dotted path (`"paperdoll.fit.slider"`) into the styles tree; missing
/// segment → `Null`.
fn jpath<'a>(root: &'a Json, path: &str) -> &'a Json {
    let mut cur = root;
    for seg in path.split('.') {
        cur = cur.get(seg).unwrap_or(&Json::Null);
    }
    cur
}

fn jnum(v: &Json, key: &str, dflt: f32) -> f32 {
    v.get(key).and_then(|n| n.as_f64()).map(|n| n as f32).unwrap_or(dflt)
}

/// First present rgba among `keys`, else `dflt`.
fn first_color(v: &Json, keys: &[&str], dflt: [f32; 4]) -> [f32; 4] {
    for key in keys {
        if let Some(a) = v.get(key).and_then(|c| c.as_array()) {
            if a.len() >= 4 {
                return std::array::from_fn(|i| a[i].as_f64().unwrap_or(0.0) as f32);
            }
        }
    }
    dflt
}

fn ptext<'a>(node: &'a UiNode, key: &str) -> Option<&'a str> {
    match node.props.get(key) {
        Some(Value::Text(t)) => Some(t),
        _ => None,
    }
}

fn pnum(node: &UiNode, key: &str) -> Option<f64> {
    match node.props.get(key) {
        Some(Value::Number(n)) => Some(*n),
        _ => None,
    }
}

fn pbool(node: &UiNode, key: &str) -> bool {
    matches!(node.props.get(key), Some(Value::Bool(true)))
}

/// Read a colour that IS a 4-array `Value` (a token-resolved rgba), else `dflt`.
/// Used for a text node's dotted `color` path (`"paperdoll.stats.color"`).
fn json_color(v: &Json, dflt: [f32; 4]) -> [f32; 4] {
    match v.as_array() {
        Some(a) if a.len() >= 4 => std::array::from_fn(|i| a[i].as_f64().unwrap_or(0.0) as f32),
        _ => dflt,
    }
}

fn eff_num(results: &ValueMap, model: &ValueMap, key: &str) -> Option<f64> {
    results.number(key).or_else(|| model.number(key))
}

fn eff_bool(results: &ValueMap, model: &ValueMap, key: &str) -> bool {
    match results.get(key) {
        Some(Value::Bool(b)) => *b,
        _ => model.is_on(key),
    }
}

fn eff_text<'a>(results: &'a ValueMap, model: &'a ValueMap, key: &str) -> Option<&'a str> {
    results.text(key).or_else(|| model.text(key))
}

#[allow(clippy::too_many_arguments)]
fn panel(r: Rect, top: [f32; 4], bot: [f32; 4], radius: f32, border: f32, border_color: [f32; 4], feather: f32) -> HudCommand {
    HudCommand::Panel {
        x: r.x,
        y: r.y,
        w: r.w,
        h: r.h,
        color: top,
        color2: bot,
        grad: if top == bot { 0.0 } else { 1.0 },
        radius,
        border,
        border_color,
        feather,
        layer: 0.0,
    }
}

fn rect_cmd(r: Rect, color: [f32; 4]) -> HudCommand {
    HudCommand::Rect { x: r.x, y: r.y, w: r.w, h: r.h, color, layer: 0.0 }
}

#[allow(clippy::too_many_arguments)]
fn push_text(out: &mut Vec<HudCommand>, x: f32, y: f32, text: &str, size: f32, color: [f32; 4], align: TextAlign, font: FontRole, italic: bool, bold: bool, tracking: f32) {
    out.push(HudCommand::Text { x, y, text: text.to_string(), size, color, layer: 0.0, align, font, italic, bold, tracking });
}

// Neutral fallbacks (only used when a style path is missing — real colour comes
// from the resolved Prism tokens in `ui_elements.json`).
const INK: [f32; 4] = [0.871, 0.847, 0.788, 1.0];
const DIM: [f32; 4] = [0.561, 0.541, 0.49, 1.0];
const PANEL: [f32; 4] = [0.078, 0.09, 0.122, 1.0];
const STONE: [f32; 4] = [0.055, 0.063, 0.086, 1.0];
const SAP: [f32; 4] = [0.141, 0.247, 0.471, 1.0];
const RUNE: [f32; 4] = [0.435, 0.592, 1.0, 1.0];

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn node(component: &str) -> UiNode {
        UiNode { component: component.to_string(), ..Default::default() }
    }

    fn prop(mut n: UiNode, k: &str, v: Value) -> UiNode {
        n.props.insert(k.to_string(), v);
        n
    }

    fn styles() -> Json {
        serde_json::json!({
            "cb": { "box": [0.1,0.1,0.1,1.0], "check": [1.0,1.0,1.0,1.0], "border": [0.2,0.2,0.2,1.0], "pad": 3 },
            "btn": { "fill_top": [0.2,0.3,0.5,1.0], "hover_top": [0.3,0.4,0.6,1.0], "label": [1.0,1.0,1.0,1.0], "border": [0.3,0.4,0.6,1.0] }
        })
    }

    // A page with one anchored column: a checkbox (bind "flag") over a button
    // (action "go"). Exercises layout (anchor + flow), hit-test (both kinds),
    // and same-frame value reflection.
    fn tree() -> UiNode {
        let cb = {
            let mut n = node("checkbox");
            n.id = "cb".into();
            n.size = Some(20.0);
            n.bind = Some("flag".into());
            n = prop(n, "box", Value::Number(14.0));
            n = prop(n, "label", Value::Text("F".into()));
            prop(n, "style", Value::Text("cb".into()))
        };
        let btn = {
            let mut n = node("button");
            n.id = "btn".into();
            n.size = Some(24.0);
            n.action = Some("go".into());
            n = prop(n, "label", Value::Text("GO".into()));
            prop(n, "style", Value::Text("btn".into()))
        };
        let mut col = node("column");
        col.anchor = Some(UiAnchor::TopLeft);
        col.offset = [16.0, 16.0];
        col.width = Some(120.0);
        col.children = vec![cb, btn];

        let mut page = node("page");
        page.children = vec![col];
        page
    }

    fn input_at(x: f32, y: f32, clicked: bool) -> UiInput {
        UiInput { mouse: Vec2::new(x, y), clicked, down: clicked, screen: Vec2::new(800.0, 600.0), typed: String::new(), backspace: false }
    }

    #[test]
    fn scroll_emits_a_viewport_clip_and_wheel_moves_the_bound_offset() {
        // A 200×100 scroll viewport holding 3 rows of 50 = 150 content → 50px max.
        let mut sc = node("scroll");
        sc.id = "sc".into();
        sc.bind = Some("sy".into());
        sc.width = Some(200.0);
        sc.height = Some(100.0);
        sc.anchor = Some(UiAnchor::TopLeft);
        sc = prop(sc, "wheel", Value::Text("wheel".into()));
        sc = prop(sc, "gutter", Value::Number(0.0)); // full-width viewport for the assertions
        for i in 0..3 {
            let mut row = node("panel");
            row.id = format!("row{i}");
            row.size = Some(50.0);
            sc.children.push(row);
        }
        let mut page = node("page");
        page.children = vec![sc];
        let styles = serde_json::json!({});

        // At rest: the content subtree is clipped to the 200×100 viewport, then reset.
        let model = ValueMap::new().with("sy", 0.0);
        let frame = run_ui(&page, &model, &styles, &input_at(-1.0, -1.0, false), &mut UiState::new());
        assert!(
            frame.commands.iter().any(|c| matches!(c, HudCommand::Clip { rect: Some(r) }
                if (r[2] - 200.0).abs() < 0.5 && (r[3] - 100.0).abs() < 0.5)),
            "scroll subtree is clipped to its viewport"
        );
        assert!(
            frame.commands.iter().any(|c| matches!(c, HudCommand::Clip { rect: None })),
            "the clip is reset after the scroll region"
        );

        // Wheel down over the region moves the bound offset, within [0, 50].
        let m2 = ValueMap::new().with("sy", 0.0).with("wheel", -1.0);
        let frame = run_ui(&page, &m2, &styles, &input_at(100.0, 50.0, false), &mut UiState::new());
        let sy = frame.results.number("sy").expect("scroll offset reported");
        assert!(sy > 0.0 && sy <= 50.0, "wheel scrolled within bounds: {sy}");

        // A large delta clamps at the content max.
        let m3 = ValueMap::new().with("sy", 0.0).with("wheel", -10.0);
        let frame = run_ui(&page, &m3, &styles, &input_at(100.0, 50.0, false), &mut UiState::new());
        assert_eq!(frame.results.number("sy"), Some(50.0), "clamped to the content max");
    }

    #[test]
    fn button_click_fires_action_and_claims_mouse() {
        let t = tree();
        let model = ValueMap::new().with("flag", false);
        let mut state = UiState::new();
        // Column at (16,16) width 120: checkbox rows y 16..36, button y 36..60.
        let frame = run_ui(&t, &model, &styles(), &input_at(50.0, 48.0, true), &mut state);
        assert!(frame.results.is_on("go"), "button action fired");
        assert!(frame.results.is_on("hud_hit"), "pointer over UI claims the mouse");
        assert!(!frame.commands.is_empty(), "something was drawn");
        assert!(!frame.results.is_on("flag"), "checkbox untouched by a button click");
    }

    #[test]
    fn checkbox_click_toggles_bound_value() {
        let t = tree();
        let model = ValueMap::new().with("flag", false);
        let mut state = UiState::new();
        // Checkbox box is the 14×14 at the column's top-left (16..30, 16..30).
        let frame = run_ui(&t, &model, &styles(), &input_at(22.0, 22.0, true), &mut state);
        assert!(frame.results.is_on("flag"), "checkbox toggled its bind on");
    }

    #[test]
    fn toggle_click_flips_bound_bool_and_stays_in_rect() {
        // A 50×25 toggle pill (dims from its style block) as the only child of a
        // page, anchored top-left so its node rect is exactly the pill's box.
        let mut tg = node("toggle");
        tg.id = "tg".into();
        tg.bind = Some("sw".into());
        tg.width = Some(50.0);
        tg.height = Some(25.0);
        tg.anchor = Some(UiAnchor::TopLeft);
        tg = prop(tg, "style", Value::Text("tg".into()));
        let mut page = node("page");
        page.children = vec![tg];

        let st = serde_json::json!({
            "tg": {
                "w": 50, "h": 25,
                "on_top": [0.14, 0.25, 0.47, 1.0], "on_bot": [0.10, 0.18, 0.36, 1.0],
                "on_border": [0.20, 0.30, 0.60, 1.0],
                "off_bg": [0.08, 0.09, 0.12, 1.0], "off_border": [0.20, 0.23, 0.28, 1.0],
                "knob_on": [0.93, 0.95, 1.0, 1.0], "knob_off": [0.56, 0.54, 0.49, 1.0]
            }
        });
        let model = ValueMap::new().with("sw", false);
        let mut state = UiState::new();

        // Off → a click anywhere on the pill (spans 0..50 × 0..25) flips it on.
        let frame = run_ui(&page, &model, &st, &input_at(25.0, 12.0, true), &mut state);
        assert!(frame.results.is_on("sw"), "toggle flipped its bind on");
        assert!(frame.results.is_on("hud_hit"), "pointer over the pill claims the mouse");

        // Every emitted panel (pill + knob) stays inside the 50×25 node rect.
        for c in &frame.commands {
            if let HudCommand::Panel { x, y, w, h, .. } = c {
                assert!(
                    *x >= -0.01 && *y >= -0.01 && x + w <= 50.01 && y + h <= 25.01,
                    "toggle geometry within node rect: {x},{y} {w}×{h}"
                );
            }
        }

        // Two-way echo: a non-click frame reports the model's current value unchanged.
        let on = ValueMap::new().with("sw", true);
        let idle = UiInput { mouse: Vec2::new(999.0, 999.0), clicked: false, down: false, screen: Vec2::new(800.0, 600.0), typed: String::new(), backspace: false };
        let frame = run_ui(&page, &on, &st, &idle, &mut state);
        assert!(frame.results.is_on("sw"), "off-pointer frame still echoes the bound bool");
    }

    #[test]
    fn radio_click_selects_its_value_and_echoes_otherwise() {
        // Two radios sharing the exclusive group key "choice"; model starts on "a".
        let radio = |id: &str, value: &str| {
            let mut n = node("radio");
            n.id = id.into();
            n.size = Some(20.0);
            n.bind = Some("choice".into());
            n = prop(n, "box", Value::Number(14.0));
            n = prop(n, "value", Value::Text(value.into()));
            n = prop(n, "label", Value::Text(value.into()));
            prop(n, "style", Value::Text("cb".into()))
        };
        let mut col = node("column");
        col.anchor = Some(UiAnchor::TopLeft);
        col.offset = [16.0, 16.0];
        col.width = Some(120.0);
        col.children = vec![radio("r_a", "a"), radio("r_b", "b")];
        let mut page = node("page");
        page.children = vec![col];

        let model = ValueMap::new().with("choice", "a");
        let mut state = UiState::new();

        // Column at (16,16): row A circle 16..30 × 16..30, row B circle 16..30 ×
        // 36..50. Click inside row B's circle → the group selects "b".
        let frame = run_ui(&page, &model, &styles(), &input_at(22.0, 42.0, true), &mut state);
        assert_eq!(frame.results.text("choice"), Some("b"), "clicking row B selects b");
        assert!(frame.results.is_on("hud_hit"), "the radio circle claims the pointer");

        // The selected row draws a filled inner dot INSIDE row B's 14×14 box.
        let dot = frame.commands.iter().find_map(|c| match c {
            HudCommand::Panel { x, y, w, h, .. } if *w < 14.0 && *h < 14.0 => Some((*x, *y, *w, *h)),
            _ => None,
        });
        let (dx, dy, dw, dh) = dot.expect("selected radio draws an inner dot");
        assert!(
            dx >= 16.0 && dy >= 36.0 && dx + dw <= 30.0 && dy + dh <= 50.0,
            "dot stays within row B's box, got ({dx},{dy},{dw},{dh})"
        );

        // A frame with the pointer off every row leaves the selection intact: each
        // row echoes the model's current value, none overwrites it with its own.
        let frame = run_ui(&page, &model, &styles(), &input_at(300.0, 300.0, false), &mut state);
        assert_eq!(frame.results.text("choice"), Some("a"), "no-click frame echoes current selection");
    }

    #[test]
    fn stepper_buttons_step_and_clamp_bound_value() {
        // A 120×24 stepper at the top-left: field spans the row, so the − end
        // button is x 0..24 and the + end button is x 96..120 (each square).
        let mut sp = node("stepper");
        sp.id = "sp".into();
        sp.bind = Some("qty".into());
        sp.width = Some(120.0);
        sp.height = Some(24.0);
        sp.anchor = Some(UiAnchor::TopLeft);
        sp = prop(sp, "min", Value::Number(0.0));
        sp = prop(sp, "max", Value::Number(10.0));
        sp = prop(sp, "step", Value::Number(1.0));
        sp = prop(sp, "decimals", Value::Number(0.0));
        let mut page = node("stepper_page");
        page.children = vec![sp];

        let st = serde_json::json!({});

        // − button (left square): 5 → 4, and the pointer claims the mouse.
        let model = ValueMap::new().with("qty", 5.0);
        let mut state = UiState::new();
        let frame = run_ui(&page, &model, &st, &input_at(12.0, 12.0, true), &mut state);
        assert_eq!(frame.results.number("qty"), Some(4.0), "− steps down by step");
        assert!(frame.results.is_on("hud_hit"), "pointer over the stepper claims the mouse");

        // + button (right square): 5 → 6.
        let frame = run_ui(&page, &model, &st, &input_at(108.0, 12.0, true), &mut state);
        assert_eq!(frame.results.number("qty"), Some(6.0), "+ steps up by step");

        // No click (pointer between the buttons) → echoes the bound value.
        let frame = run_ui(&page, &model, &st, &input_at(60.0, 12.0, false), &mut state);
        assert_eq!(frame.results.number("qty"), Some(5.0), "reports current value each frame");

        // Clamp at the floor: 0 − step stays at min.
        let lo = ValueMap::new().with("qty", 0.0);
        let frame = run_ui(&page, &lo, &st, &input_at(12.0, 12.0, true), &mut state);
        assert_eq!(frame.results.number("qty"), Some(0.0), "clamped at min");

        // Clamp at the ceiling: 10 + step stays at max.
        let hi = ValueMap::new().with("qty", 10.0);
        let frame = run_ui(&page, &hi, &st, &input_at(108.0, 12.0, true), &mut state);
        assert_eq!(frame.results.number("qty"), Some(10.0), "clamped at max");
    }

    #[test]
    fn stepper_geometry_stays_within_node_rect() {
        let mut sp = node("stepper");
        sp.id = "sp".into();
        sp.bind = Some("qty".into());
        sp.width = Some(120.0);
        sp.height = Some(24.0);
        sp.anchor = Some(UiAnchor::TopLeft);
        sp = prop(sp, "min", Value::Number(0.0));
        sp = prop(sp, "max", Value::Number(10.0));
        let mut page = node("stepper_page");
        page.children = vec![sp];

        let model = ValueMap::new().with("qty", 5.0);
        let mut state = UiState::new();
        let frame = run_ui(&page, &model, &serde_json::json!({}), &input_at(0.0, 0.0, false), &mut state);
        // Every drawn box (field + both end buttons) lands inside the 120×24 rect.
        for c in &frame.commands {
            if let HudCommand::Rect { x, y, w, h, .. } = c {
                assert!(
                    *x >= -0.01 && *y >= -0.01 && x + w <= 120.01 && y + h <= 24.01,
                    "stepper rect within node: x={x} y={y} w={w} h={h}"
                );
            }
        }
    }

    #[test]
    fn pill_toggle_click_selects_segment_value() {
        let opt = |value: &str, label: &str| {
            let n = prop(node("option"), "value", Value::Text(value.into()));
            prop(n, "label", Value::Text(label.into()))
        };
        let mut pill = node("pill_toggle");
        pill.id = "pt".into();
        pill.bind = Some("mode".into());
        pill.width = Some(180.0);
        pill.height = Some(30.0);
        pill.anchor = Some(UiAnchor::TopLeft);
        pill = prop(pill, "style", Value::Text("pill".into()));
        // Options are CHILD data nodes (value+label), not placed sub-widgets.
        pill.children = vec![opt("low", "Low"), opt("med", "Med"), opt("high", "High")];

        let mut page = node("page");
        page.children = vec![pill];

        let styles = serde_json::json!({
            "pill": {
                "bg": [0.05, 0.06, 0.08, 1.0], "border": [0.2, 0.2, 0.2, 1.0],
                "radius": 15, "pad": 3, "h": 30,
                "active_top": [0.14, 0.25, 0.47, 1.0], "active_bot": [0.10, 0.18, 0.34, 1.0],
                "active_label": [0.9, 0.9, 0.95, 1.0], "label": [0.5, 0.5, 0.5, 1.0], "label_size": 11
            }
        });
        let model = ValueMap::new().with("mode", "low");
        let mut state = UiState::new();

        // Pill at (0,0) 180×30. Inner strip x 3..177 (174 wide) → 3 cells of 58:
        // low 3..61, med 61..119, high 119..177. Click the middle cell → "med".
        let frame = run_ui(&page, &model, &styles, &input_at(90.0, 15.0, true), &mut state);
        assert_eq!(frame.results.text("mode"), Some("med"), "middle segment selects its value");
        assert!(frame.results.is_on("hud_hit"), "pointer over the pill claims the mouse");

        // Every drawn panel stays within the 180×30 node rect (well + highlight).
        for c in &frame.commands {
            if let HudCommand::Panel { x, y, w, h, .. } = c {
                assert!(
                    *x >= 0.0 && *y >= 0.0 && x + w <= 180.5 && y + h <= 30.5,
                    "pill panel within node rect, got {x},{y},{w},{h}"
                );
            }
        }

        // No click → echoes the current selection unchanged (two-way sync each frame).
        let frame = run_ui(&page, &model, &styles, &input_at(90.0, 15.0, false), &mut state);
        assert_eq!(frame.results.text("mode"), Some("low"), "non-click frame reports current value");

        // A click OUTSIDE every cell leaves the selection untouched.
        let frame = run_ui(&page, &model, &styles, &input_at(300.0, 15.0, true), &mut state);
        assert_eq!(frame.results.text("mode"), Some("low"), "a miss doesn't change the value");
    }

    #[test]
    fn tabs_click_selects_value_and_defaults_to_first() {
        // Three tabs (a|b|c) bound to "tab", a 300×30 strip at the origin: three
        // even 100px cells. Children are pure data carriers (value + label).
        let mk = |value: &str, label: &str| {
            let mut t = node("tab");
            t = prop(t, "value", Value::Text(value.into()));
            prop(t, "label", Value::Text(label.into()))
        };
        let mut tabs = node("tabs");
        tabs.id = "tabs".into();
        tabs.bind = Some("tab".into());
        tabs.width = Some(300.0);
        tabs.height = Some(30.0);
        tabs.anchor = Some(UiAnchor::TopLeft);
        tabs = prop(tabs, "tab_active", Value::Text("ta".into()));
        tabs = prop(tabs, "tab_idle", Value::Text("ti".into()));
        tabs.children = vec![mk("a", "A"), mk("b", "B"), mk("c", "C")];
        let mut page = node("tabs_page");
        page.children = vec![tabs];

        let st = serde_json::json!({
            "ta": { "fill_top": [0.2,0.3,0.5,1.0], "label": [1.0,1.0,1.0,1.0] },
            "ti": { "fill_top": [0.09,0.10,0.13,1.0], "label": [0.56,0.54,0.49,1.0] }
        });
        let model = ValueMap::new();
        let mut state = UiState::new();

        // Click the middle cell (x 100..200) → selects its value "b".
        let frame = run_ui(&page, &model, &st, &input_at(150.0, 15.0, true), &mut state);
        assert_eq!(frame.results.text("tab"), Some("b"), "clicking tab 2 writes its value");
        assert!(frame.results.is_on("hud_hit"), "pointer over the strip claims the mouse");

        // No prior value + pointer off the strip → reports the first tab (a strip
        // always has one active tab), and claims nothing.
        let frame = run_ui(&page, &model, &st, &input_at(400.0, 400.0, false), &mut state);
        assert_eq!(frame.results.text("tab"), Some("a"), "unset bind defaults to the first tab");
        assert!(!frame.results.is_on("hud_hit"), "pointer off the strip doesn't claim the mouse");

        // Every drawn cell / label stays within the 300×30 strip rect.
        for c in &frame.commands {
            match c {
                HudCommand::Panel { x, y, w, h, .. } => assert!(
                    *x >= -0.01 && *y >= -0.01 && x + w <= 300.01 && y + h <= 30.01,
                    "tab cell within strip: x={x} y={y} w={w} h={h}"
                ),
                HudCommand::Text { x, y, .. } => assert!(
                    *x >= -0.01 && *x <= 300.01 && *y >= -0.01 && *y <= 30.01,
                    "tab label within strip: x={x} y={y}"
                ),
                _ => {}
            }
        }
    }

    fn select_styles_json() -> Json {
        serde_json::json!({
            "controls": {
                "field": { "top": [0.0,0.0,0.0,1.0], "bot": [0.0,0.0,0.0,1.0], "border": [0.2,0.2,0.2,1.0], "radius": 3, "h": 40, "label": [1.0,1.0,1.0,1.0], "label_size": 15, "caret": [0.5,0.5,1.0,1.0] },
                "menu": { "top": [0.1,0.1,0.1,1.0], "bot": [0.0,0.0,0.0,1.0], "border": [0.2,0.2,0.2,1.0], "radius": 3, "row_h": 30, "label": [1.0,1.0,1.0,1.0], "label_size": 15, "sel_bg": [0.2,0.3,0.5,1.0], "sel_label": [1.0,1.0,1.0,1.0], "hover_bg": [0.1,0.15,0.25,1.0] }
            }
        })
    }

    // A select (bind "mode") over two option children. A field click opens the
    // popup (into `state.open`); an option click writes that option's `value` and
    // closes; the closed field's panel stays within the node rect.
    fn select_tree() -> UiNode {
        let opt = |val: &str, label: &str| {
            let mut n = node("option");
            n = prop(n, "value", Value::Text(val.into()));
            prop(n, "label", Value::Text(label.into()))
        };
        let mut sel = node("select");
        sel.id = "sel".into();
        sel.bind = Some("mode".into());
        sel.width = Some(200.0);
        sel.height = Some(40.0);
        sel.anchor = Some(UiAnchor::TopLeft);
        sel = prop(sel, "placeholder", Value::Text("Choose…".into()));
        sel = prop(sel, "style", Value::Text("controls".into()));
        sel.children = vec![opt("a", "Alpha"), opt("b", "Beta")];
        let mut page = node("page");
        page.children = vec![sel];
        page
    }

    #[test]
    fn select_click_opens_then_option_click_writes_bind_and_closes() {
        let t = select_tree();
        let styles = select_styles_json();
        let model = ValueMap::new(); // no initial selection
        let mut state = UiState::new();

        // Closed: idle pointer far away. The field panel fills the node rect exactly
        // and is the ONLY panel (no popup rows drawn while closed).
        let f0 = run_ui(&t, &model, &styles, &input_at(400.0, 400.0, false), &mut state);
        assert!(state.open.is_none(), "starts closed");
        let panels: Vec<_> = f0
            .commands
            .iter()
            .filter_map(|c| match c {
                HudCommand::Panel { x, y, w, h, .. } => Some((*x, *y, *w, *h)),
                _ => None,
            })
            .collect();
        assert_eq!(panels, vec![(0.0, 0.0, 200.0, 40.0)], "closed = just the field panel, within the node rect");

        // Click the field (0..200 × 0..40) → opens into state.open, writes nothing yet.
        let f1 = run_ui(&t, &model, &styles, &input_at(100.0, 20.0, true), &mut state);
        assert_eq!(state.open.as_deref(), Some("sel"), "clicking the field opens the menu");
        assert!(f1.results.is_on("hud_hit"), "the field claims the pointer");

        // Menu open (state persists). Rows start at y = 40 + 6 = 46, row_h 30:
        // row 0 = 46..76 (Alpha), row 1 = 76..106 (Beta). Click Beta.
        let f2 = run_ui(&t, &model, &styles, &input_at(100.0, 90.0, true), &mut state);
        assert_eq!(f2.results.text("mode"), Some("b"), "clicking Beta writes its value");
        assert!(state.open.is_none(), "picking an option closes the menu");

        // A click outside a re-opened menu just closes it (writes nothing new).
        run_ui(&t, &model, &styles, &input_at(100.0, 20.0, true), &mut state); // re-open
        assert_eq!(state.open.as_deref(), Some("sel"));
        run_ui(&t, &model, &styles, &input_at(600.0, 500.0, true), &mut state); // click far outside
        assert!(state.open.is_none(), "a click outside closes the menu");
    }

    #[test]
    fn select_open_menu_rows_are_lifted_above_the_field() {
        let t = select_tree();
        let styles = select_styles_json();
        let model = ValueMap::new().with("mode", "a");
        let mut state = UiState::new();
        // Force it open, then draw: the field is layer 0, the popup panel + rows layer 1.
        run_ui(&t, &model, &styles, &input_at(100.0, 20.0, true), &mut state);
        assert_eq!(state.open.as_deref(), Some("sel"));
        let frame = run_ui(&t, &model, &styles, &input_at(0.0, 0.0, false), &mut state);
        // First panel = field (layer 0); a later panel = the popup (layer 1).
        let panel_layers: Vec<f32> = frame
            .commands
            .iter()
            .filter_map(|c| match c {
                HudCommand::Panel { layer, .. } => Some(*layer),
                _ => None,
            })
            .collect();
        assert_eq!(panel_layers.first(), Some(&0.0), "field sits on the base layer");
        assert!(panel_layers.contains(&1.0), "popup panel is lifted a sub-layer");
    }

    #[test]
    fn text_field_focus_type_backspace_and_click_away() {
        // A single text_field (bind "name") anchored at (10,10), 200×40.
        let mut tf = node("text_field");
        tf.id = "name_field".into();
        tf.width = Some(200.0);
        tf.height = Some(40.0);
        tf.anchor = Some(UiAnchor::TopLeft);
        tf.offset = [10.0, 10.0];
        tf.bind = Some("name".into());
        tf = prop(tf, "placeholder", Value::Text("enter name".into()));
        tf = prop(tf, "style", Value::Text("field".into()));
        let mut page = node("page");
        page.children = vec![tf];

        let styles = serde_json::json!({
            "field": {
                "top": [0.02, 0.02, 0.03, 1.0], "bot": [0.04, 0.04, 0.05, 1.0],
                "border": [0.2, 0.2, 0.2, 1.0], "hover_border": [0.5, 0.4, 0.2, 1.0],
                "caret": [0.43, 0.59, 1.0, 1.0], "radius": 3, "h": 40,
                "label": [0.9, 0.9, 0.85, 1.0], "label_size": 15
            }
        });
        let mut state = UiState::new();

        // Click inside the well → focuses the field and claims the mouse.
        let model = ValueMap::new().with("name", "");
        let f = run_ui(&page, &model, &styles, &input_at(100.0, 30.0, true), &mut state);
        assert!(f.results.is_on("hud_hit"), "a click in the well claims the mouse");
        assert_eq!(state.focus.as_deref(), Some("name_field"), "click focuses the field");

        // Type two chars on a non-click frame → appended to the bound string. Feed
        // the prior frame's result back as the model, as the engine would.
        let model = ValueMap::new().with("name", f.results.text("name").unwrap_or("").to_string());
        let mut typing = input_at(100.0, 30.0, false);
        typing.typed = "Hi".into();
        let f = run_ui(&page, &model, &styles, &typing, &mut state);
        assert_eq!(f.results.text("name"), Some("Hi"), "typed chars append to the value");

        // Backspace → pops the last char.
        let model = ValueMap::new().with("name", f.results.text("name").unwrap().to_string());
        let mut bs = input_at(100.0, 30.0, false);
        bs.backspace = true;
        let f = run_ui(&page, &model, &styles, &bs, &mut state);
        assert_eq!(f.results.text("name"), Some("H"), "backspace pops the last char");

        // Well geometry stays within the node rect (10,10 .. 210,50), and while
        // focused a caret rect is emitted inside it.
        let well = f.commands.iter().find_map(|c| match c {
            HudCommand::Panel { x, y, w, h, .. } => Some((*x, *y, *w, *h)),
            _ => None,
        }).expect("well drawn");
        assert!(well.0 >= 10.0 && well.1 >= 10.0 && well.0 + well.2 <= 210.0 && well.1 + well.3 <= 50.0, "well within node rect: {well:?}");
        let caret = f.commands.iter().any(|c| matches!(c, HudCommand::Rect { x, .. } if *x >= 10.0 && *x <= 210.0));
        assert!(caret, "a caret is drawn inside the well while focused");

        // Click OUTSIDE the well → de-focuses; the value survives.
        let model = ValueMap::new().with("name", f.results.text("name").unwrap().to_string());
        let f = run_ui(&page, &model, &styles, &input_at(400.0, 300.0, true), &mut state);
        assert!(state.focus.is_none(), "clicking away clears focus");
        assert_eq!(f.results.text("name"), Some("H"), "value preserved on click-away");

        // A keystroke while unfocused is ignored.
        let model = ValueMap::new().with("name", "H");
        let mut typing = input_at(400.0, 300.0, false);
        typing.typed = "X".into();
        let f = run_ui(&page, &model, &styles, &typing, &mut state);
        assert_eq!(f.results.text("name"), Some("H"), "typing is ignored when unfocused");
    }

    /// A button's caption resolves through the same `text_bind` channel every other text-bearing
    /// node uses, so an exclusive choice or a state-dependent action can label itself from the
    /// Model instead of needing one node per possible caption.
    #[test]
    fn button_label_can_come_from_the_model() {
        let mut b = node("button");
        b.id = "b".into();
        b.size = Some(24.0);
        b = prop(b, "text_bind", Value::Text("caption".into()));
        b = prop(b, "label", Value::Text("FALLBACK".into()));
        b = prop(b, "style", Value::Text("btn".into()));

        let model = ValueMap::new().with("caption", "\u{25c9}  Skin");
        let mut state = UiState::new();
        let f = run_ui(&b, &model, &styles(), &input_at(-9.0, -9.0, false), &mut state);
        let drew = f.commands.iter().any(
            |c| matches!(c, HudCommand::Text { text, .. } if text.contains("Skin")),
        );
        assert!(drew, "the bound caption reached the draw commands: {:?}", f.commands);

        // With no bind, the literal label still wins — existing buttons are unaffected.
        let plain = prop(node("button"), "label", Value::Text("GO".into()));
        let f = run_ui(&plain, &ValueMap::new(), &styles(), &input_at(-9.0, -9.0, false), &mut state);
        assert!(f
            .commands
            .iter()
            .any(|c| matches!(c, HudCommand::Text { text, .. } if text == "GO")));
    }

    /// `color_bind` names a Model key holding a dotted style path, so a row whose STATE decides
    /// its colour resolves through one node rather than one node per possible colour.
    #[test]
    fn text_colour_can_follow_a_bound_style_path() {
        let styles = serde_json::json!({
            "map": { "ok": [0.0, 1.0, 0.0, 1.0], "review": [1.0, 1.0, 0.0, 1.0] }
        });
        let mut t = node("text");
        t.size = Some(20.0);
        t = prop(t, "text", Value::Text("thigh_l".into()));
        t = prop(t, "color_bind", Value::Text("row_color".into()));

        let mut state = UiState::new();
        let green = ValueMap::new().with("row_color", "map.ok");
        let f = run_ui(&t, &green, &styles, &input_at(-9.0, -9.0, false), &mut state);
        let color = f.commands.iter().find_map(|c| match c {
            HudCommand::Text { color, .. } => Some(*color),
            _ => None,
        });
        assert_eq!(color, Some([0.0, 1.0, 0.0, 1.0]), "resolved through the bound path");

        // The SAME node, a different bound path → a different colour. One node, N states.
        let amber = ValueMap::new().with("row_color", "map.review");
        let f = run_ui(&t, &amber, &styles, &input_at(-9.0, -9.0, false), &mut state);
        let color = f.commands.iter().find_map(|c| match c {
            HudCommand::Text { color, .. } => Some(*color),
            _ => None,
        });
        assert_eq!(color, Some([1.0, 1.0, 0.0, 1.0]));
    }

    /// A node's whole `style` can ride a bound Model path (`style_bind`), so ONE panel switches
    /// between an active and an idle look from state — how the non-interactive pipeline tabs light
    /// the current step without a stack of visibility-toggled panels.
    #[test]
    fn panel_style_can_follow_a_bound_path() {
        let styles = serde_json::json!({
            "tab_active": { "fill_top": [0.1, 0.2, 0.4, 1.0], "fill_bot": [0.1, 0.2, 0.4, 1.0] },
            "tab_idle":   { "fill_top": [0.2, 0.2, 0.2, 1.0], "fill_bot": [0.2, 0.2, 0.2, 1.0] }
        });
        let mut t = node("panel");
        t.width = Some(80.0);
        t.height = Some(24.0);
        t.anchor = Some(UiAnchor::TopLeft);
        t = prop(t, "style_bind", Value::Text("tab_style".into()));
        // A literal fallback the bind overrides — proves the bind wins when the key is present.
        t = prop(t, "style", Value::Text("tab_idle".into()));

        let mut state = UiState::new();
        let panel_fill = |f: &UiFrame| {
            f.commands.iter().find_map(|c| match c {
                HudCommand::Panel { color, .. } => Some(*color),
                _ => None,
            })
        };

        let active = ValueMap::new().with("tab_style", "tab_active");
        let f = run_ui(&t, &active, &styles, &input_at(-9.0, -9.0, false), &mut state);
        assert_eq!(panel_fill(&f), Some([0.1, 0.2, 0.4, 1.0]), "bound path selects the active style");

        // The SAME node, a different bound value → the idle style. One node, N states.
        let idle = ValueMap::new().with("tab_style", "tab_idle");
        let f = run_ui(&t, &idle, &styles, &input_at(-9.0, -9.0, false), &mut state);
        assert_eq!(panel_fill(&f), Some([0.2, 0.2, 0.2, 1.0]), "a different bound value → idle");

        // With the bound key absent, the literal `style` fallback still draws — existing nodes are
        // unaffected by the new channel.
        let f = run_ui(&t, &ValueMap::new(), &styles, &input_at(-9.0, -9.0, false), &mut state);
        assert_eq!(panel_fill(&f), Some([0.2, 0.2, 0.2, 1.0]), "no bound value → literal style");
    }

    #[test]
    fn drag_source_picks_up_payload_and_reports_drop() {
        // Any component kind can be a drag source — it is prop-driven, not a new kind.
        let mut row = node("panel");
        row.id = "row".into();
        row = prop(row, "drag_kind", Value::Text("clip".into()));
        row = prop(row, "drag_id", Value::Text("walk_forward".into()));

        let model = ValueMap::new();
        let screen = Vec2::new(200.0, 100.0);
        let mut state = UiState::new();

        // Press inside the source → payload picked up, active while held.
        let press = UiInput { mouse: Vec2::new(50.0, 50.0), clicked: true, down: true, screen, typed: String::new(), backspace: false };
        let f = run_ui(&row, &model, &styles(), &press, &mut state);
        assert_eq!(f.results.text("drag_kind"), Some("clip"));
        assert_eq!(f.results.text("drag_id"), Some("walk_forward"));
        assert!(f.results.is_on("drag_active"), "held drag is active");
        assert!(!f.results.is_on("drag_dropped"), "not dropped while still held");
        assert_eq!(state.drag().map(|d| d.id.as_str()), Some("walk_forward"));

        // Still held, cursor moved — the payload is retained across frames.
        let hold = UiInput { mouse: Vec2::new(180.0, 90.0), clicked: false, down: true, screen, typed: String::new(), backspace: false };
        let f = run_ui(&row, &model, &styles(), &hold, &mut state);
        assert!(f.results.is_on("drag_active"));
        assert_eq!(f.results.text("drag_id"), Some("walk_forward"));

        // Release → exactly one drop edge carrying the payload, then the drag clears.
        let release = UiInput { mouse: Vec2::new(180.0, 90.0), clicked: false, down: false, screen, typed: String::new(), backspace: false };
        let f = run_ui(&row, &model, &styles(), &release, &mut state);
        assert!(f.results.is_on("drag_dropped"), "release reports the drop");
        assert_eq!(f.results.text("drag_id"), Some("walk_forward"), "drop carries the payload");
        assert!(state.drag().is_none(), "drag clears after the drop");

        // A node without `drag_kind` never picks anything up.
        let mut plain = UiState::new();
        let f = run_ui(&node("panel"), &model, &styles(), &press, &mut plain);
        assert!(f.results.text("drag_kind").is_none());
        assert!(plain.drag().is_none());
    }

    #[test]
    fn slider_drag_writes_bound_value_and_captures() {
        let mut sl = node("slider");
        sl.id = "s".into();
        sl.bind = Some("v".into());
        sl.width = Some(200.0);
        sl.height = Some(20.0);
        sl.anchor = Some(UiAnchor::TopLeft);
        sl = prop(sl, "slider_h", Value::Number(12.0));
        sl = prop(sl, "min", Value::Number(0.0));
        sl = prop(sl, "max", Value::Number(100.0));
        let mut page = node("page");
        page.children = vec![sl];

        let st = serde_json::json!({});
        let model = ValueMap::new().with("v", 0.0);
        let mut state = UiState::new();
        // Track spans the full 200px width from x=0; press at the midpoint.
        let frame = run_ui(&page, &model, &st, &input_at(100.0, 10.0, true), &mut state);
        let v = frame.results.number("v").expect("slider wrote its bind");
        assert!((v - 50.0).abs() < 2.0, "midpoint press ≈ 50, got {v}");
        assert!(frame.results.is_on("hud_hit"));

        // Still held, cursor moved right → keeps updating even off-track.
        let held = UiInput { mouse: Vec2::new(180.0, 10.0), clicked: false, down: true, screen: Vec2::new(800.0, 600.0), typed: String::new(), backspace: false };
        let frame = run_ui(&page, &model, &st, &held, &mut state);
        assert!(frame.results.number("v").unwrap() > 80.0, "drag keeps writing");
    }

    #[test]
    fn hidden_subtree_is_not_placed() {
        let mut hidden = node("button");
        hidden.action = Some("nope".into());
        hidden.visible_bind = Some("shown".into());
        hidden.size = Some(24.0);
        hidden.anchor = Some(UiAnchor::TopLeft);
        let mut page = node("page");
        page.children = vec![hidden];

        let model = ValueMap::new().with("shown", false);
        let mut state = UiState::new();
        let frame = run_ui(&page, &model, &serde_json::json!({}), &input_at(5.0, 5.0, true), &mut state);
        assert!(!frame.results.is_on("nope"), "a hidden button can't be clicked");

        let model = ValueMap::new().with("shown", true);
        let frame = run_ui(&page, &model, &serde_json::json!({}), &input_at(5.0, 5.0, true), &mut state);
        assert!(frame.results.is_on("nope"), "shown → clickable");
    }

    #[test]
    fn sprite_aspect_locks_square_and_sits_below_a_layered_panel() {
        // A viewport-tall Muse sprite (tex 4, height = 114% of the 600px screen,
        // width aspect-locked square) on the base layer, and a popup panel lifted
        // to layer 1 above it.
        let mut muse = node("sprite");
        muse.anchor = Some(UiAnchor::BottomRight);
        muse = prop(muse, "tex", Value::Number(4.0));
        muse = prop(muse, "height_frac", Value::Number(1.14));
        muse = prop(muse, "aspect", Value::Number(1.0)); // square → width follows height

        let mut popup = node("panel");
        popup.anchor = Some(UiAnchor::Center);
        popup.width = Some(200.0);
        popup.height = Some(100.0);
        popup = prop(popup, "layer", Value::Number(1.0));
        popup = prop(popup, "style", Value::Text("btn".into())); // any style → a panel bg

        let mut page = node("page");
        page.children = vec![muse, popup];

        let model = ValueMap::new();
        let mut state = UiState::new();
        let frame = run_ui(&page, &model, &styles(), &input_at(0.0, 0.0, false), &mut state);

        // The sprite blits tex 4 at 1.14 × the 600px screen height = 684px, square, layer 0.
        let (tex, w, h, slayer) = frame
            .commands
            .iter()
            .find_map(|c| match c {
                HudCommand::Sprite { tex, w, h, layer, .. } => Some((*tex, *w, *h, *layer)),
                _ => None,
            })
            .expect("sprite drawn");
        assert_eq!(tex, 4);
        assert!((h - 684.0).abs() < 0.5, "height = 1.14×600, got {h}");
        assert!((w - h).abs() < 0.5, "aspect=1 keeps the Muse square, got w={w} h={h}");
        assert_eq!(slayer, 0.0, "the Muse stays on the base layer");

        // The popup panel is lifted a whole layer above the backdrop sprite.
        let panel_layer = frame
            .commands
            .iter()
            .find_map(|c| match c {
                HudCommand::Panel { layer, .. } => Some(*layer),
                _ => None,
            })
            .expect("popup panel drawn");
        assert_eq!(panel_layer, 1.0, "popup subtree lifts above the backdrop");
    }

    #[test]
    fn stage_node_reserves_an_inset_slot_and_draws_its_own_backdrop() {
        let st = serde_json::json!({
            "thumb": { "fill_top": [0.1,0.1,0.1,1.0], "fill_bot": [0.0,0.0,0.0,1.0],
                       "border": [0.2,0.2,0.2,1.0], "border_w": 1, "radius": 6, "inset": 2 },
            "warm": [1.0, 0.9, 0.8, 1.0]
        });
        let mut s = node("stage");
        s.id = "pack_thumb".into();
        s.anchor = Some(UiAnchor::TopLeft);
        s.offset = [10.0, 20.0];
        s.width = Some(92.0);
        s.height = Some(92.0);
        s = prop(s, "style", Value::Text("thumb".into()));
        s = prop(s, "source", Value::Text("portrait".into()));
        s = prop(s, "tint", Value::Text("warm".into()));
        let mut page = node("page");
        page.children = vec![s];

        let mut state = UiState::new();
        let frame = run_ui(&page, &ValueMap::new(), &st, &input_at(50.0, 60.0, false), &mut state);

        assert_eq!(frame.stages.len(), 1, "one PiP slot reserved");
        let slot = &frame.stages[0];
        assert_eq!(slot.id, "pack_thumb");
        assert_eq!(slot.source, "portrait");
        // The image rect is the node rect inset by the STYLE's `inset` — so a whole
        // family of stages shares one inset without repeating it per node.
        assert_eq!((slot.x, slot.y, slot.w, slot.h), (12.0, 22.0, 88.0, 88.0));
        assert_eq!(slot.tint, [1.0, 0.9, 0.8, 1.0], "tint resolved from its dotted path");
        assert!(slot.live, "a stage with no liveness policy renders");
        // The walker draws the backdrop itself, which is why the scene passes
        // `frame: None` to composite_panel — one panel, one code path.
        assert!(
            frame.commands.iter().any(|c| matches!(c, HudCommand::Panel { .. })),
            "stage drew its panel backdrop"
        );
        assert!(frame.results.is_on("hud_hit"), "a stage claims the pointer");
    }

    #[test]
    fn stage_liveness_follows_its_bind_and_a_sourceless_stage_reserves_nothing() {
        let st = serde_json::json!({ "thumb": { "fill_top": [0.1,0.1,0.1,1.0] } });
        let staged = |id: &str, live_key: &str| {
            let mut s = node("stage");
            s.id = id.into();
            s.anchor = Some(UiAnchor::TopLeft);
            s.width = Some(40.0);
            s.height = Some(40.0);
            s = prop(s, "style", Value::Text("thumb".into()));
            s = prop(s, "source", Value::Text("portrait".into()));
            prop(s, "live_bind", Value::Text(live_key.into()))
        };
        // A stage with no `source` is dropped rather than reserving a broken slot.
        let mut orphan = node("stage");
        orphan.id = "orphan".into();
        orphan.width = Some(10.0);
        orphan.height = Some(10.0);

        let mut page = node("page");
        page.children = vec![staged("hot", "sel"), staged("cold", "unsel"), orphan];

        let model = ValueMap::new().with("sel", true).with("unsel", false);
        let mut state = UiState::new();
        let frame = run_ui(&page, &model, &st, &input_at(700.0, 500.0, false), &mut state);

        assert_eq!(frame.stages.len(), 2, "the source-less stage reserved nothing");
        let live_of = |id: &str| frame.stages.iter().find(|s| s.id == id).unwrap().live;
        assert!(live_of("hot"), "bound true → renders a fresh target");
        assert!(!live_of("cold"), "bound false → scene reuses its cached poster");
    }

    #[test]
    fn rune_corners_draws_four_glyphs_glow_top_bronze_bottom() {
        // A rune_corners overlay filling a 200×120 rect at the origin, carrying the four
        // corner glyphs + the reused `runes` style block (mirrors `settings.runes`).
        let mut rc = node("rune_corners");
        rc.id = "rc".into();
        rc.width = Some(200.0);
        rc.height = Some(120.0);
        rc.anchor = Some(UiAnchor::TopLeft);
        rc = prop(rc, "tl", Value::Text("ᛞ".into()));
        rc = prop(rc, "tr", Value::Text("ᛝ".into()));
        rc = prop(rc, "bl", Value::Text("ᚨ".into()));
        rc = prop(rc, "br", Value::Text("ᛟ".into()));
        rc = prop(rc, "style", Value::Text("runes".into()));
        let mut page = node("page");
        page.children = vec![rc];

        let glow: [f32; 4] = [0.43, 0.59, 1.0, 1.0];
        let bronze: [f32; 4] = [0.5, 0.38, 0.2, 1.0];
        let styles = serde_json::json!({
            "runes": { "size": 16, "inset": 12, "top": glow, "bot": bronze }
        });
        let model = ValueMap::new();
        let mut state = UiState::new();

        let frame = run_ui(&page, &model, &styles, &input_at(400.0, 400.0, false), &mut state);

        // Exactly four Rune-font glyphs, emitted in TL, TR, BL, BR order.
        let runes: Vec<(&str, f32, f32, [f32; 4])> = frame
            .commands
            .iter()
            .filter_map(|c| match c {
                HudCommand::Text { text, x, y, color, font: FontRole::Rune, .. } => {
                    Some((text.as_str(), *x, *y, *color))
                }
                _ => None,
            })
            .collect();
        assert_eq!(runes.len(), 4, "four corner glyphs drawn");
        assert_eq!(runes[0].0, "ᛞ", "top-left glyph");
        assert_eq!(runes[1].0, "ᛝ", "top-right glyph");
        assert_eq!(runes[2].0, "ᚨ", "bottom-left glyph");
        assert_eq!(runes[3].0, "ᛟ", "bottom-right glyph");

        // Top pair glows (rune-light); bottom pair is dim bronze.
        assert_eq!(runes[0].3, glow, "top-left uses the glow colour");
        assert_eq!(runes[1].3, glow, "top-right uses the glow colour");
        assert_eq!(runes[2].3, bronze, "bottom-left uses the bronze colour");
        assert_eq!(runes[3].3, bronze, "bottom-right uses the bronze colour");

        // Left pair anchors at the left inset; right pair anchors at the right inset
        // (right-aligned). Every glyph anchor sits inside the 200×120 node rect.
        assert!((runes[0].1 - 12.0).abs() < 0.01 && (runes[2].1 - 12.0).abs() < 0.01, "left glyphs at inset 12");
        assert!((runes[1].1 - 188.0).abs() < 0.01 && (runes[3].1 - 188.0).abs() < 0.01, "right glyphs at w - inset");
        for &(g, x, y, _) in &runes {
            assert!((0.0..=200.0).contains(&x) && (0.0..=120.0).contains(&y), "glyph {g} anchor within rect: ({x},{y})");
        }

        // Top pair sits above the bottom pair (a corner decoration, not a pile).
        assert!(runes[0].2 < runes[2].2 && runes[1].2 < runes[3].2, "top pair above bottom pair");

        // No interaction: a bare decoration doesn't claim the pointer on its own.
        assert!(!frame.results.is_on("hud_hit"), "a bare rune_corners overlay claims nothing");
    }

    #[test]
    fn tooltip_paints_name_meta_and_a_coloured_rune_without_claiming_the_pointer() {
        // A floating info card the SCENE positions (rect) and gates (visible_bind):
        // an element rune badge, a name headline, and a dim meta line. Presentational
        // — it must never claim the pointer, or a cursor-following tip eats every click.
        let mut tip = node("tooltip");
        tip.id = "tip".into();
        tip.width = Some(220.0);
        tip.height = Some(64.0);
        tip.anchor = Some(UiAnchor::TopLeft);
        tip.offset = [20.0, 20.0];
        tip = prop(tip, "style", Value::Text("tip".into()));
        tip = prop(tip, "name", Value::Text("Emberlash".into()));
        tip = prop(tip, "rune", Value::Text("\u{16A0}".into())); // ᚠ Elder Futhark 'fehu'
        tip = prop(tip, "rune_color", Value::Text("elem.fire".into()));
        tip = prop(tip, "meta", Value::Text("evocation · 1 action · 3 mana".into()));

        let styles = serde_json::json!({
            "elem": { "fire": [1.0, 0.4, 0.1, 1.0] },
            "tip": {
                "bg": [0.05, 0.06, 0.09, 0.94], "border": [0.17, 0.19, 0.24, 1.0],
                "radius": 5, "pad": 10,
                "name_color": [0.9, 0.9, 0.85, 1.0], "name_size": 16,
                "meta_color": [0.5, 0.5, 0.5, 1.0], "meta_size": 12
            }
        });
        let model = ValueMap::new();
        let mut state = UiState::new();

        let mut page = node("page");
        page.children = vec![tip.clone()];

        // Click squarely over the card (rect 20,20 .. 240,84; centre ≈ 130,52) — a
        // presentational tip claims nothing.
        let frame = run_ui(&page, &model, &styles, &input_at(130.0, 52.0, true), &mut state);
        assert!(!frame.results.is_on("hud_hit"), "a presentational tooltip never steals the pointer");

        // The backdrop panel fills the node rect (the single Panel command).
        let panel = frame.commands.iter().find_map(|c| match c {
            HudCommand::Panel { x, y, w, h, .. } => Some((*x, *y, *w, *h)),
            _ => None,
        });
        assert_eq!(panel, Some((20.0, 20.0, 220.0, 64.0)), "card backdrop fills the node rect");

        // Name headline drawn in the Display face.
        let name = frame.commands.iter().find_map(|c| match c {
            HudCommand::Text { text, font, y, .. } if text == "Emberlash" => Some((*font, *y)),
            _ => None,
        }).expect("name headline drawn");
        assert_eq!(name.0, FontRole::Display, "the name uses the display face");

        // Meta line drawn in the dim meta colour, on the row below the name.
        let meta = frame.commands.iter().find_map(|c| match c {
            HudCommand::Text { text, color, y, .. } if text.contains("evocation") => Some((*color, *y)),
            _ => None,
        }).expect("meta line drawn");
        assert_eq!(meta.0, [0.5, 0.5, 0.5, 1.0], "meta uses the dim meta_color");
        assert!(meta.1 > name.1, "meta sits on the line below the name");

        // Element rune drawn in the Rune face, coloured by its dotted rune_color path.
        let rune = frame.commands.iter().find_map(|c| match c {
            HudCommand::Text { text, font, color, .. } if *font == FontRole::Rune => Some((text.clone(), *color)),
            _ => None,
        }).expect("rune glyph drawn in the rune face");
        assert_eq!(rune.0.as_str(), "\u{16A0}", "the glyph is the node's rune");
        assert_eq!(rune.1, [1.0, 0.4, 0.1, 1.0], "rune colour resolved from the dotted rune_color path");

        // Every glyph origin sits inside the node rect (20,20 .. 240,84).
        for c in &frame.commands {
            if let HudCommand::Text { x, y, .. } = c {
                assert!(*x >= 20.0 && *y >= 20.0 && *x <= 240.0 && *y <= 84.0, "text within card: {x},{y}");
            }
        }

        // The rune is OPTIONAL — the same card with no `rune` still draws name + meta,
        // and emits no Rune-face glyph.
        let mut plain = tip;
        plain.props.remove("rune");
        let mut page2 = node("page");
        page2.children = vec![plain];
        let frame = run_ui(&page2, &model, &styles, &input_at(-9.0, -9.0, false), &mut state);
        assert!(
            frame.commands.iter().any(|c| matches!(c, HudCommand::Text { text, .. } if text == "Emberlash")),
            "name still drawn without a rune"
        );
        assert!(
            !frame.commands.iter().any(|c| matches!(c, HudCommand::Text { font, .. } if *font == FontRole::Rune)),
            "no rune glyph when the prop is absent"
        );
    }

    // Add inside `mod tests`, alongside the other piece tests. Uses inline style json.
    #[test]
    fn badge_draws_toned_pill_and_claims_pointer() {
        // A presentational badge: an accent-tone chip labelled "NEW", 60×20 at the origin.
        let mut b = node("badge");
        b.id = "b".into();
        b.width = Some(60.0);
        b.height = Some(20.0);
        b.anchor = Some(UiAnchor::TopLeft);
        b = prop(b, "label", Value::Text("NEW".into()));
        b = prop(b, "tone", Value::Text("accent".into()));
        b = prop(b, "style", Value::Text("badge".into()));
        let mut page = node("page");
        page.children = vec![b];

        let st = serde_json::json!({
            "badge": {
                "pad": 0, "h": 20, "radius": 10, "label_size": 11,
                "accent_bg": [0.14, 0.25, 0.47, 1.0], "accent_label": [0.93, 0.95, 1.0, 1.0],
                "neutral_bg": [0.08, 0.09, 0.12, 1.0], "neutral_label": [0.56, 0.54, 0.49, 1.0],
                "bronze_bg": [0.43, 0.35, 0.20, 1.0], "bronze_label": [0.87, 0.85, 0.79, 1.0],
                "solid_bg": [0.72, 0.59, 0.35, 1.0], "solid_label": [0.03, 0.04, 0.05, 1.0]
            }
        });
        let model = ValueMap::new();
        let mut state = UiState::new();

        // Pointer over the pill → the badge claims the mouse (scene can't pick through).
        let frame = run_ui(&page, &model, &st, &input_at(30.0, 10.0, true), &mut state);
        assert!(frame.results.is_on("hud_hit"), "pointer over the badge claims the mouse");

        // The pill uses the accent tone's bg, a pill radius (≈ h/2), and stays inside the
        // 60×20 node rect.
        let pill = frame
            .commands
            .iter()
            .find_map(|c| match c {
                HudCommand::Panel { x, y, w, h, color, radius, .. } => Some((*x, *y, *w, *h, *color, *radius)),
                _ => None,
            })
            .expect("badge drew its pill");
        assert_eq!(pill.4, [0.14, 0.25, 0.47, 1.0], "accent tone fills with accent_bg");
        assert!((pill.5 - 10.0).abs() < 0.01, "radius ≈ h/2 (a full capsule)");
        assert!(
            pill.0 >= -0.01 && pill.1 >= -0.01 && pill.0 + pill.2 <= 60.01 && pill.1 + pill.3 <= 20.01,
            "pill within the node rect: {pill:?}"
        );

        // The centred label reached the draw commands in the accent label colour.
        let label = frame
            .commands
            .iter()
            .find_map(|c| match c {
                HudCommand::Text { text, color, align, .. } => Some((text.clone(), *color, *align)),
                _ => None,
            })
            .expect("badge drew its label");
        assert_eq!(label.0, "NEW");
        assert_eq!(label.1, [0.93, 0.95, 1.0, 1.0], "label uses accent_label");
        assert!(matches!(label.2, TextAlign::Center), "label is centred");

        // `solid` OVERRIDES the tone → a filled bronze chip (solid_bg / solid_label),
        // even though an `accent` tone is also present.
        let mut b2 = node("badge");
        b2.width = Some(60.0);
        b2.height = Some(20.0);
        b2.anchor = Some(UiAnchor::TopLeft);
        b2 = prop(b2, "label", Value::Text("LIVE".into()));
        b2 = prop(b2, "tone", Value::Text("accent".into())); // tone present…
        b2 = prop(b2, "solid", Value::Bool(true)); // …but solid wins
        b2 = prop(b2, "style", Value::Text("badge".into()));
        let mut page2 = node("page");
        page2.children = vec![b2];
        let frame = run_ui(&page2, &model, &st, &input_at(500.0, 500.0, false), &mut state);
        let fill = frame
            .commands
            .iter()
            .find_map(|c| match c {
                HudCommand::Panel { color, .. } => Some(*color),
                _ => None,
            })
            .expect("solid badge drew its pill");
        assert_eq!(fill, [0.72, 0.59, 0.35, 1.0], "solid overrides tone → solid_bg (bronze)");
    }

    // ── Add inside `mod tests`, alongside the other per-kind tests. Uses the same
    // `node` / `prop` / `input_at` / `run_ui` harness the existing tests use.
        #[test]
        fn context_menu_row_click_fires_action_skips_divider_and_disabled() {
            // Items are CHILD data nodes: a plain row (+keybind hint), an active row, a
            // divider, a disabled row, and a final plain row. row_h 30 → five 30px slots
            // stacked from the top of the menu rect.
            let mut cut = prop(node("item"), "label", Value::Text("Cut".into()));
            cut.action = Some("cut".into());
            cut = prop(cut, "hint", Value::Text("X".into()));

            let mut copy = prop(node("item"), "label", Value::Text("Copy".into()));
            copy.action = Some("copy".into());
            copy = prop(copy, "active", Value::Bool(true));

            let divider = prop(node("item"), "divider", Value::Bool(true));

            let mut paste = prop(node("item"), "label", Value::Text("Paste".into()));
            paste.action = Some("paste".into());
            paste = prop(paste, "disabled", Value::Bool(true));

            let mut del = prop(node("item"), "label", Value::Text("Delete".into()));
            del.action = Some("del".into());

            let mut menu = node("context_menu");
            menu.id = "ctx".into();
            menu.width = Some(200.0);
            menu.height = Some(150.0);
            menu.anchor = Some(UiAnchor::TopLeft);
            menu = prop(menu, "style", Value::Text("menu".into()));
            menu.children = vec![cut, copy, divider, paste, del];

            let mut page = node("page");
            page.children = vec![menu];

            // The reused settings.controls.menu block shape (values inline, not a live path).
            let styles = serde_json::json!({
                "menu": {
                    "top": [0.1,0.1,0.1,1.0], "bot": [0.0,0.0,0.0,1.0],
                    "border": [0.2,0.2,0.2,1.0], "radius": 3, "row_h": 30, "label_size": 15,
                    "label": [1.0,1.0,1.0,1.0], "sel_bg": [0.2,0.3,0.5,1.0],
                    "sel_label": [1.0,1.0,1.0,1.0], "hover_bg": [0.1,0.15,0.25,1.0]
                }
            });
            let model = ValueMap::new();
            let mut state = UiState::new();

            // Row 0 (y 0..30) is live → fires "cut" and the menu claims the pointer.
            let f = run_ui(&page, &model, &styles, &input_at(100.0, 15.0, true), &mut state);
            assert!(f.results.is_on("cut"), "clicking a live row fires its action");
            assert!(f.results.is_on("hud_hit"), "the menu surface claims the pointer");

            // Row 3 (y 90..120) is disabled → its action never fires, but the surface
            // still claims the pointer (no pick-through to the scene behind).
            let f = run_ui(&page, &model, &styles, &input_at(100.0, 105.0, true), &mut state);
            assert!(!f.results.is_on("paste"), "a disabled row is not clickable");
            assert!(f.results.is_on("hud_hit"), "still claims the pointer over a disabled row");

            // Row 2 (y 60..90) is a divider → inert; nothing fires.
            let f = run_ui(&page, &model, &styles, &input_at(100.0, 75.0, true), &mut state);
            assert!(
                !f.results.is_on("cut") && !f.results.is_on("copy") && !f.results.is_on("paste") && !f.results.is_on("del"),
                "a divider row fires no action"
            );

            // Row 4 (y 120..150) is live → fires "del".
            let f = run_ui(&page, &model, &styles, &input_at(100.0, 135.0, true), &mut state);
            assert!(f.results.is_on("del"), "the last row fires its action");

            // Idle draw frame: pointer over row 0. Every drawn panel / row band / hairline /
            // label stays within the 200×150 node rect, and the active row draws its wash.
            let f = run_ui(&page, &model, &styles, &input_at(100.0, 15.0, false), &mut state);
            for c in &f.commands {
                match c {
                    HudCommand::Panel { x, y, w, h, .. } => assert!(
                        *x >= -0.01 && *y >= -0.01 && x + w <= 200.01 && y + h <= 150.01,
                        "menu panel within node rect: {x},{y} {w}×{h}"
                    ),
                    HudCommand::Rect { x, y, w, h, .. } => assert!(
                        *x >= -0.01 && *y >= -0.01 && x + w <= 200.01 && y + h <= 150.01,
                        "row wash / hairline within node rect: {x},{y} {w}×{h}"
                    ),
                    HudCommand::Text { x, y, .. } => assert!(
                        *x >= -0.01 && *x <= 200.01 && *y >= -0.01 && *y <= 150.01,
                        "menu text within node rect: {x},{y}"
                    ),
                    _ => {}
                }
            }
            let has_sel = f
                .commands
                .iter()
                .any(|c| matches!(c, HudCommand::Rect { color, .. } if *color == [0.2, 0.3, 0.5, 1.0]));
            assert!(has_sel, "the active row draws a selection wash (sel_bg)");

            // A click fully OUTSIDE the menu fires nothing and claims nothing.
            let f = run_ui(&page, &model, &styles, &input_at(400.0, 400.0, true), &mut state);
            assert!(!f.results.is_on("cut") && !f.results.is_on("del"), "a click off the menu fires nothing");
            assert!(!f.results.is_on("hud_hit"), "a click off the menu doesn't claim the pointer");
        }

    // Keep HashMap import used even if the struct-literal path changes.
    #[allow(dead_code)]
    fn _uses_hashmap() -> HashMap<String, Value> {
        HashMap::new()
    }
}
