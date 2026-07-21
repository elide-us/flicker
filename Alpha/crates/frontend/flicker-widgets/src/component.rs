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
}

/// Retained interaction state the caller holds across frames (currently the set
/// of sliders capturing the mouse mid-drag, keyed by node id/bind). A slider drag
/// keeps updating — and keeps claiming the mouse — until the button releases,
/// even if the cursor leaves the track.
#[derive(Default)]
pub struct UiState {
    dragging: HashSet<String>,
}

impl UiState {
    /// A fresh, empty interaction state.
    pub fn new() -> Self {
        Self::default()
    }
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
        Rect { x: self.x + p, y: self.y + p, w: self.w - 2.0 * p, h: self.h - 2.0 * p }
    }
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
    resolve(tree, screen, model, 0.0, &mut placed);

    // Hit-test pass: fold events + value edits into `results`, drag into `state`.
    let mut results = ValueMap::new();
    let mut hud_hit = false;
    for p in &placed {
        hit_node(p, model, input, state, styles, &mut results, &mut hud_hit);
    }
    if !state.dragging.is_empty() {
        hud_hit = true;
    }
    results.set("hud_hit", hud_hit);

    // Draw pass: values reflect this frame's edits (results override model).
    let mut commands = Vec::new();
    for p in &placed {
        let start = commands.len();
        draw_node(p, model, &results, styles, input, &mut commands);
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

    UiFrame { commands, results }
}

// ── Layout ───────────────────────────────────────────────────────────────────

fn resolve<'a>(node: &'a UiNode, rect: Rect, model: &ValueMap, layer: f32, out: &mut Vec<Placed<'a>>) {
    if !visible(node, model) {
        return;
    }
    // A node's optional `layer` prop accumulates down the tree, so a whole
    // subtree (a styled popup + its buttons + labels) can sit above a backdrop.
    let layer = layer + pnum(node, "layer").map(|n| n as f32).unwrap_or(0.0);
    out.push(Placed { node, rect, enabled: enabled(node, model), layer });
    if node.children.is_empty() {
        return;
    }
    let inner = rect.inset(node.pad);
    match node.component.as_str() {
        "row" => flow(node, inner, model, layer, out, true),
        "column" | "panel" => flow(node, inner, model, layer, out, false),
        // page / stack / anything else: overlay children, each placed by its own anchor.
        _ => {
            for c in &node.children {
                if !visible(c, model) {
                    continue;
                }
                let r = anchored(c, inner, model);
                resolve(c, r, model, layer, out);
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
        resolve(c, r, model, layer, out);
        pos += len + node.gap;
    }
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
                node.pad * 2.0 + gaps + kids.iter().map(|c| child_main(c, model, true)).sum::<f32>()
            });
            let h = node.height.unwrap_or_else(|| {
                node.pad * 2.0
                    + kids.iter().map(|c| child_cross(c, model, true)).fold(0.0, f32::max)
            });
            Vec2::new(w, h)
        }
        "column" | "panel" => {
            let h = node.height.unwrap_or_else(|| {
                node.pad * 2.0
                    + gaps
                    + kids.iter().map(|c| child_main(c, model, false)).sum::<f32>()
            });
            let w = node.width.unwrap_or_else(|| {
                node.pad * 2.0
                    + kids.iter().map(|c| child_cross(c, model, false)).fold(0.0, f32::max)
            });
            Vec2::new(w, h)
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
        "slider" => {
            let id = slider_id(node);
            let (track, grab) = slider_rects(node, r);
            let hovering = r.contains(input.mouse);
            if hovering {
                *hud_hit = true;
                // Clicking the track GRABS it for dragging (the padded grab region).
                if input.clicked && grab.contains(input.mouse) {
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
        // A styled container (a panel) claims the pointer, so a click on the
        // panel background doesn't pick through to the scene.
        "row" | "column" | "panel" | "stack" | "page"
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
    out: &mut Vec<HudCommand>,
) {
    let node = p.node;
    let r = p.rect;
    let st = style_of(node, styles);
    match node.component.as_str() {
        "row" | "column" | "panel" | "stack" | "page" => {
            if !st.is_null() {
                draw_panel_bg(r, st, out);
            }
        }
        "text" => {
            let text = node_text(node, model, results);
            // Font size: an explicit `text_size` prop, else the node's layout height
            // (a single line is usually its own height), else a default.
            let size = pnum(node, "text_size").map(|n| n as f32).or(node.size).unwrap_or(14.0);
            // Colour: a dotted `color` path into a token-resolved rgba (text's escape
            // hatch, since colours can't ride as scalar props), else the style block.
            let color = match ptext(node, "color") {
                Some(path) => json_color(jpath(styles, path), INK),
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
            push_text(out, x, r.y, &text, size, color, align, node_font(node));
        }
        "button" => {
            let hovered = r.contains(input.mouse);
            draw_button(r, st, node, hovered, out);
        }
        "cell" => draw_cell(r, node, model, results, styles, out),
        "checkbox" => draw_checkbox(r, node, model, results, st, out),
        "slider" => draw_slider(r, node, model, results, st, out),
        "sprite" => draw_sprite(r, node, out),
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

fn draw_button(r: Rect, st: &Json, node: &UiNode, hovered: bool, out: &mut Vec<HudCommand>) {
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
    let label = ptext(node, "label").unwrap_or_default();
    let lc = if hovered {
        first_color(st, &["hover_label", "label"], INK)
    } else {
        first_color(st, &["label"], INK)
    };
    let lsz = jnum(st, "label_size", pnum(node, "label_size").map(|n| n as f32).unwrap_or(14.0));
    push_text(out, r.x + r.w * 0.5, r.y + (r.h - lsz) * 0.5, label, lsz, lc, TextAlign::Center, FontRole::Label);
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
    push_text(out, r.x + r.w * 0.5, r.y + (r.h - lsz) * 0.5, label, lsz, lc, TextAlign::Center, FontRole::Label);
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
        push_text(out, lx, bx.y + (bx.h - lsz) * 0.5, label, lsz, INK, TextAlign::Left, FontRole::Body);
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
        push_text(out, r.x, r.y + (r.h - lsz) * 0.5, label, lsz, lc, TextAlign::Left, FontRole::Body);
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
        push_text(out, r.x + r.w, r.y + (r.h - vsz) * 0.5, &fmt_val(value as f64, node), vsz, first_color(st, &["value_color"], DIM), TextAlign::Right, FontRole::Body);
    }
}

// ── Geometry helpers ─────────────────────────────────────────────────────────

fn checkbox_box(node: &UiNode, r: Rect) -> Rect {
    let b = pnum(node, "box").map(|n| n as f32).unwrap_or(14.0);
    Rect { x: r.x, y: r.y, w: b, h: b }
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

fn fmt_val(v: f64, node: &UiNode) -> String {
    let dec = pnum(node, "decimals").unwrap_or(2.0) as usize;
    let sign = if pbool(node, "plus") && v >= 0.0 { "+" } else { "" };
    let suffix = ptext(node, "suffix").unwrap_or("");
    format!("{sign}{v:.dec$}{suffix}")
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
fn push_text(out: &mut Vec<HudCommand>, x: f32, y: f32, text: &str, size: f32, color: [f32; 4], align: TextAlign, font: FontRole) {
    out.push(HudCommand::Text { x, y, text: text.to_string(), size, color, layer: 0.0, align, font });
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
        UiInput { mouse: Vec2::new(x, y), clicked, down: clicked, screen: Vec2::new(800.0, 600.0) }
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
        let held = UiInput { mouse: Vec2::new(180.0, 10.0), clicked: false, down: true, screen: Vec2::new(800.0, 600.0) };
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

    // Keep HashMap import used even if the struct-literal path changes.
    #[allow(dead_code)]
    fn _uses_hashmap() -> HashMap<String, Value> {
        HashMap::new()
    }
}
