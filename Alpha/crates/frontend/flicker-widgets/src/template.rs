//! The **template tier** — the middle of `Scene → Template → Element`.
//!
//! A *piece* is one leaf `component` the walker draws (`button`, `slider`, …). A
//! *template* is a **named Rust builder** that composes pieces into a reusable
//! arrangement (a Workbench, a PopupMenu, a ChoiceDialog). A *scene* declares a
//! tree of placed nodes — some plain pieces, some `template` references with
//! `slots` filled — as DATA; [`expand`] turns every `template` node into the
//! subtree its builder produces, once, before the tree is cached and walked.
//!
//! This crate owns builders because a builder emits [`UiNode`]s — the widget
//! layer's job. `flicker-script` only *parses* the tree (it gained `template` +
//! `slots` as plain-data fields); it never learns what a template is, so the
//! crate stack stays `flicker-widgets → flicker-script`, never the reverse.
//!
//! Builders are **pure**: they read scalar props + move named slots into place
//! and emit nodes whose `style` prop names a dotted path into `theme.tokens`.
//! They never touch a colour, so a template cannot fork the one palette.

use std::collections::HashMap;

use flicker_script::{UiAnchor, UiNode, Value};

/// Named child groups a scene fills for a template (`header`, `body`, …). The
/// builder `remove`s the ones it places; anything left is simply dropped.
pub type Slots = HashMap<String, Vec<UiNode>>;

/// Context handed to a builder. `id_prefix` is the template node's own `id`, so a
/// builder can derive stable child ids when it needs them (reserved; most
/// builders ignore it).
pub struct BuildCtx {
    pub id_prefix: String,
}

/// A template builder: compose pieces into ONE subtree, consuming named slots.
/// Kept a plain `fn` (no captured state, no lifetimes) so the registry is a flat
/// function table and builders stay unit-testable in isolation.
pub type TemplateFn = fn(&BuildCtx, &HashMap<String, Value>, &mut Slots) -> UiNode;

/// The name → builder table. A flat `HashMap` is the lightest thing that is still
/// a registry: [`builtin_templates`] is the one place a template is registered,
/// and a scene crate can `.insert()` a bespoke builder before it calls
/// [`expand`] — without editing this crate.
pub type TemplateRegistry = HashMap<&'static str, TemplateFn>;

/// The built-in template set. Add a template in exactly one place: here.
pub fn builtin_templates() -> TemplateRegistry {
    let mut m: TemplateRegistry = HashMap::new();
    m.insert("workbench", workbench as TemplateFn);
    m.insert("window", window as TemplateFn);
    m.insert("card", card as TemplateFn);
    m.insert("popup_menu", popup_menu as TemplateFn);
    m.insert("choice_dialog", choice_dialog as TemplateFn);
    m.insert("side_by_side_rtt", side_by_side_rtt as TemplateFn);
    m.insert("quad_rtt_view", quad_rtt_view as TemplateFn);
    m
}

/// Expand every `template` node into the subtree its builder produces, depth-first
/// (post-order): a node's `children` and `slots` are expanded **before** the node
/// itself, so a template that emits another template still resolves. A
/// template-free tree is returned UNCHANGED (structural identity) — the reason a
/// scene not yet using templates is completely unaffected when `expand` is wired
/// into its load path.
pub fn expand(mut node: UiNode, reg: &TemplateRegistry) -> UiNode {
    node.children = node.children.into_iter().map(|c| expand(c, reg)).collect();
    let mut slots: Slots = std::mem::take(&mut node.slots);
    for group in slots.values_mut() {
        let done: Vec<UiNode> = std::mem::take(group)
            .into_iter()
            .map(|c| expand(c, reg))
            .collect();
        *group = done;
    }

    let Some(name) = node.template.take() else {
        // An ordinary piece/container — returned verbatim.
        node.slots = slots;
        return node;
    };

    match reg.get(name.as_str()) {
        Some(builder) => {
            let ctx = BuildCtx {
                id_prefix: node.id.clone(),
            };
            let mut built = builder(&ctx, &node.props, &mut slots);
            overlay_placement(&mut built, &node);
            built
        }
        None => {
            // Best-effort, like every other loader here: warn and stand in an empty
            // page so a typo'd template name fails visibly, not with a panic.
            tracing::warn!("ui arrangement: unknown template `{name}`");
            UiNode {
                component: "page".to_string(),
                id: node.id,
                anchor: node.anchor,
                offset: node.offset,
                size: node.size,
                grow: node.grow,
                width: node.width,
                height: node.height,
                ..Default::default()
            }
        }
    }
}

/// Copy the template node's own placement onto the builder's root **only where the
/// builder left it default** — so a scene can pin / size / gate a template
/// instance (`{ template = "choice_dialog", anchor = "center" }`) while a builder
/// that sets its own layout keeps it.
fn overlay_placement(built: &mut UiNode, src: &UiNode) {
    if built.anchor.is_none() {
        built.anchor = src.anchor;
    }
    if built.offset == [0.0, 0.0] {
        built.offset = src.offset;
    }
    if built.size.is_none() {
        built.size = src.size;
    }
    if built.grow.is_none() {
        built.grow = src.grow;
    }
    if built.width.is_none() {
        built.width = src.width;
    }
    if built.height.is_none() {
        built.height = src.height;
    }
    if built.id.is_empty() {
        built.id = src.id.clone();
    }
    if built.visible_bind.is_none() {
        built.visible_bind = src.visible_bind.clone();
    }
}

// ── builder helpers ──────────────────────────────────────────────────────────

/// A fresh leaf/container node of `component` kind.
fn elem(component: &str) -> UiNode {
    UiNode {
        component: component.to_string(),
        ..Default::default()
    }
}

/// Set a node's dotted `style` path (a prop, resolved against `theme.tokens` at
/// draw time). Colours never cross here — only the path name.
fn with_style(mut n: UiNode, style: Option<&str>) -> UiNode {
    if let Some(s) = style {
        n.props.insert("style".to_string(), Value::Text(s.to_string()));
    }
    n
}

/// Set a numeric prop (e.g. `width_frac`).
fn with_num(mut n: UiNode, key: &str, v: f64) -> UiNode {
    n.props.insert(key.to_string(), Value::Number(v));
    n
}

/// Set a text prop (e.g. `align` / `font` / `color` / `text`) — the string-valued
/// analog of [`with_num`], so a builder can chain scalar props onto a node.
fn with_text(mut n: UiNode, key: &str, v: &str) -> UiNode {
    n.props.insert(key.to_string(), Value::Text(v.to_string()));
    n
}

fn p_num(p: &HashMap<String, Value>, key: &str) -> Option<f64> {
    match p.get(key) {
        Some(Value::Number(n)) => Some(*n),
        _ => None,
    }
}

fn p_text<'a>(p: &'a HashMap<String, Value>, key: &str) -> Option<&'a str> {
    match p.get(key) {
        Some(Value::Text(s)) => Some(s.as_str()),
        _ => None,
    }
}

/// Read a boolean prop (absent / non-bool / literal `false` → `false`). Shared by
/// every builder that reads a scene flag (`has_min`, `disabled`, `divider`, …);
/// the string-and-number sibling of [`p_text`] / [`p_num`].
fn p_bool(p: &HashMap<String, Value>, key: &str) -> bool {
    matches!(p.get(key), Some(Value::Bool(true)))
}

/// Pull a named slot's nodes out (leaving it empty), or an empty vec if absent.
fn take_slot(slots: &mut Slots, key: &str) -> Vec<UiNode> {
    slots.remove(key).unwrap_or_default()
}

// ── templates ────────────────────────────────────────────────────────────────

/// **Workbench** — the full-screen editor bench: a full-bleed header bar, a
/// (Loomforge-style) tab strip, a work area of `viewport` + a fixed `rail`, and a
/// full-bleed footer bar. Promotes the Kilnworks `hud_assetpipeline.lua` shape;
/// the `viewport` slot takes a `QuadRTTView` / `SideBySideRTT` (or any content).
///
/// Slots: `header`, `tabs`, `viewport`, `rail`, `footer`. Props (all optional):
/// `header_h/pad/gap` + `header_style`, `tab_h/pad/gap` + `tab_style`,
/// `body_pad/gap`, `footer_h/pad/gap` + `footer_btn_h` + `footer_style`. The
/// numeric params come from the scene (its `ui_elements` section) so the bar
/// heights stay single-sourced with the rest of its layout.
fn workbench(_ctx: &BuildCtx, p: &HashMap<String, Value>, slots: &mut Slots) -> UiNode {
    let mut header = with_style(elem("row"), p_text(p, "header_style"));
    header.height = p_num(p, "header_h").map(|n| n as f32);
    header.pad = p_num(p, "header_pad").unwrap_or(0.0) as f32;
    header.gap = p_num(p, "header_gap").unwrap_or(0.0) as f32;
    header.children = take_slot(slots, "header");

    let mut tabs = with_style(elem("row"), p_text(p, "tab_style"));
    tabs.height = p_num(p, "tab_h").map(|n| n as f32);
    tabs.pad = p_num(p, "tab_pad").unwrap_or(0.0) as f32;
    tabs.gap = p_num(p, "tab_gap").unwrap_or(0.0) as f32;
    tabs.children = take_slot(slots, "tabs");

    // Work area: the framed viewport takes the leftover width, the rail sits beside
    // it (first-class panels, not floating over the viewport).
    let mut body = elem("row");
    body.grow = Some(1.0);
    body.pad = p_num(p, "body_pad").unwrap_or(0.0) as f32;
    body.gap = p_num(p, "body_gap").unwrap_or(0.0) as f32;
    let mut body_children = take_slot(slots, "viewport");
    body_children.extend(take_slot(slots, "rail"));
    body.children = body_children;

    // Footer: a styled bar (panel) wrapping a button row, matching the header.
    let mut footer_row = elem("row");
    footer_row.gap = p_num(p, "footer_gap").unwrap_or(0.0) as f32;
    footer_row.height = p_num(p, "footer_btn_h").map(|n| n as f32);
    footer_row.children = take_slot(slots, "footer");
    let mut footer = with_style(elem("panel"), p_text(p, "footer_style"));
    footer.height = p_num(p, "footer_h").map(|n| n as f32);
    footer.pad = p_num(p, "footer_pad").unwrap_or(0.0) as f32;
    footer.gap = p_num(p, "footer_gap").unwrap_or(0.0) as f32;
    footer.children = vec![footer_row];

    // Root: one full-screen column, top to bottom.
    let mut col = elem("column");
    col.anchor = Some(UiAnchor::TopLeft);
    col = with_num(col, "width_frac", 1.0);
    col = with_num(col, "height_frac", 1.0);
    col.children = vec![header, tabs, body, footer];
    col
}

/// **Window** — the canonical carved-stone frame (a DS `Window`): a styled frame
/// box wrapping a title bar (title · optional minimize / close controls), a
/// content **well**, and an optional footer, with the four corner **rune** inlays
/// overlaid on the frame. Generalises the `settings.lua` window chrome (title bar
/// + body + footer + corner-rune dots) into one reusable builder.
///
/// The frame is a **`stack`** — a styled box that OVERLAYS its children (a `panel`
/// would *flow* them top-to-bottom, stacking the runes below the chrome instead of
/// over it) — so the `rune_corners` inlays share the frame's rect and sit over the
/// chrome column. A styled `stack` draws through the same `draw_panel_bg` path as a
/// panel, so it IS "the frame bg" in look. Placed inside a `page` / `stack` scene
/// root, its `Center` anchor + `w` / `h` float it as a modal; as a bare root the
/// walker gives it the full screen.
///
/// Slots: `content` (the body), `footer` (the control row — the footer bar is
/// omitted entirely when this slot is empty). Props (all optional): `title` (bar
/// caption) + `title_size`; `w` / `h` (frame size); `style` (dotted-path PREFIX,
/// default `"settings"` → `settings.window` / `.titlebar` / `.titlebar.close` /
/// `.runes`); `close_action` (default `"close"`) + `has_close` (default true);
/// `min_action` (default `"minimize"`) + `has_min`; `close_label` / `min_label`;
/// plus scene-supplied numerics `title_h` / `title_pad` / `title_gap`, `btn_w`,
/// `body_pad` + `body_style`, `footer_h` / `footer_pad` / `footer_gap` +
/// `footer_style`. The bar / well / footer styles come from the scene by dotted
/// path — no colour ever crosses here.
fn window(_ctx: &BuildCtx, p: &HashMap<String, Value>, slots: &mut Slots) -> UiNode {
    // Dotted-path PREFIX → the reused style blocks (settings.* by default).
    let prefix = p_text(p, "style").unwrap_or("settings").to_string();
    let sty = |leaf: &str| format!("{prefix}.{leaf}");

    // ── title bar: title · flexible spacer · [minimize] · [close] ──
    let mut title = with_style(elem("text"), Some(sty("titlebar").as_str()));
    if let Some(t) = p_text(p, "title") {
        title.props.insert("text".to_string(), Value::Text(t.to_string()));
    }
    title
        .props
        .insert("text_size".to_string(), Value::Number(p_num(p, "title_size").unwrap_or(22.0)));

    // A grow spacer eats the leftover width, pushing the window controls hard-right.
    let mut spacer = elem("stack");
    spacer.grow = Some(1.0);

    let btn_w = p_num(p, "btn_w").unwrap_or(40.0) as f32;
    let mut bar = vec![title, spacer];
    if p_bool(p, "has_min") {
        let min_style = p_text(p, "min_style").map(|s| s.to_string()).unwrap_or_else(|| sty("titlebar.close"));
        let mut min_btn = with_style(elem("button"), Some(min_style.as_str()));
        min_btn.size = Some(btn_w); // main-axis (width) in the row; height fills the bar
        min_btn.action = Some(p_text(p, "min_action").unwrap_or("minimize").to_string());
        min_btn.props.insert("label".to_string(), Value::Text(p_text(p, "min_label").unwrap_or("–").to_string()));
        bar.push(min_btn);
    }
    // Close shows by default (the canonical control); a scene drops it with has_close = false.
    if !matches!(p.get("has_close"), Some(Value::Bool(false))) {
        let close_style = p_text(p, "close_style").map(|s| s.to_string()).unwrap_or_else(|| sty("titlebar.close"));
        let mut close_btn = with_style(elem("button"), Some(close_style.as_str()));
        close_btn.size = Some(btn_w);
        close_btn.action = Some(p_text(p, "close_action").unwrap_or("close").to_string());
        close_btn.props.insert("label".to_string(), Value::Text(p_text(p, "close_label").unwrap_or("×").to_string()));
        bar.push(close_btn);
    }

    let mut titlebar = with_style(elem("row"), Some(sty("titlebar").as_str()));
    titlebar.height = Some(p_num(p, "title_h").unwrap_or(52.0) as f32);
    // Per-axis inset: a wide horizontal pad, but only a small vertical one, so the bar
    // keeps its full height and the title + close centre vertically in it. A uniform
    // `pad` this wide would exceed half a 52px bar and collapse its inner height.
    titlebar.pad_x = Some(p_num(p, "title_pad").unwrap_or(16.0) as f32);
    titlebar.pad_y = Some(p_num(p, "title_pad_y").unwrap_or(8.0) as f32);
    titlebar.gap = p_num(p, "title_gap").unwrap_or(10.0) as f32;
    titlebar.children = bar;

    // ── content well: a padded box that grows to fill the body between the bars.
    // Unstyled by default so the frame fill shows through (like settings.lua); a
    // scene opts into a recessed fill with `body_style`.
    let mut well = with_style(elem("panel"), p_text(p, "body_style"));
    well.grow = Some(1.0);
    well.pad = p_num(p, "body_pad").unwrap_or(0.0) as f32;
    well.children = take_slot(slots, "content");

    // ── chrome column: title bar · content well · [footer] ──
    let mut chrome = vec![titlebar, well];
    let footer_slot = take_slot(slots, "footer");
    if !footer_slot.is_empty() {
        // Footer = a styled bar (panel) wrapping a button row (mirrors `workbench`).
        let mut footer_row = elem("row");
        footer_row.grow = Some(1.0);
        footer_row.gap = p_num(p, "footer_gap").unwrap_or(12.0) as f32;
        footer_row.children = footer_slot;
        let mut footer = with_style(elem("panel"), p_text(p, "footer_style"));
        footer.height = Some(p_num(p, "footer_h").unwrap_or(58.0) as f32);
        // Per-axis inset (see the title bar): wide sides, short top/bottom, so the button
        // row keeps the bar's full height instead of a uniform pad collapsing it.
        footer.pad_x = Some(p_num(p, "footer_pad").unwrap_or(16.0) as f32);
        footer.pad_y = Some(p_num(p, "footer_pad_y").unwrap_or(10.0) as f32);
        footer.children = vec![footer_row];
        chrome.push(footer);
    }

    let mut col = elem("column");
    col.anchor = Some(UiAnchor::TopLeft);
    col = with_num(col, "width_frac", 1.0);
    col = with_num(col, "height_frac", 1.0);
    col.children = chrome;

    // ── rune-inlay overlay: fills the frame; glyphs at the four corners ──
    let mut runes = with_style(elem("rune_corners"), Some(sty("runes").as_str()));
    runes.anchor = Some(UiAnchor::TopLeft);
    runes = with_num(runes, "width_frac", 1.0);
    runes = with_num(runes, "height_frac", 1.0);

    // ── frame: a styled STACK overlaying the chrome column + the rune inlays ──
    let mut frame = with_style(elem("stack"), Some(sty("window").as_str()));
    frame.anchor = Some(UiAnchor::Center);
    frame.width = p_num(p, "w").map(|n| n as f32);
    frame.height = p_num(p, "h").map(|n| n as f32);
    frame.children = vec![col, runes];
    frame
}

/// **Card** — a Prism carved-stone slab: a styled `panel` wrapping a `column`
/// of an optional header (a `title` line above an optional `subtitle` line) and
/// then the `content` slot. A DS Card is inert chrome, so `disabled` does NOT
/// gate interactivity — it re-points the header text at muted colour paths
/// (`menu.desc` / `menu.meta`) so a locked card reads greyed without needing a
/// second style block, following the walker's "route it through the one style
/// channel" rule (a colour never crosses into Rust here — only the dotted path).
///
/// Slot: `content` (the card body the scene supplies). Props (all optional):
/// `title` / `subtitle` (each line is emitted only when its prop is present),
/// `disabled` (bool — dims the header), `style` (the panel style path, default
/// `menu.panel`), plus sizing knobs `pad` / `gap` / `header_gap` /
/// `title_size` / `subtitle_size` (so a scene single-sources them like workbench).
fn card(_ctx: &BuildCtx, p: &HashMap<String, Value>, slots: &mut Slots) -> UiNode {
    let disabled = p_bool(p, "disabled");

    // One header line: a `text` piece whose `color`/`font` ride as props (a colour
    // is a dotted path the walker resolves at draw, never an rgba baked in here).
    // `size` sets BOTH the line's layout height and its font size — a single line
    // is its own height — matching the walker's text measure/draw fallbacks.
    let line = |text: &str, size: f64, color: &str, font: &str| -> UiNode {
        let mut n = elem("text");
        n.size = Some(size as f32);
        n.props.insert("text".to_string(), Value::Text(text.to_string()));
        n.props.insert("color".to_string(), Value::Text(color.to_string()));
        n.props.insert("font".to_string(), Value::Text(font.to_string()));
        n
    };

    // Header: title (display face) over subtitle (Cinzel-caps label face). Each is
    // present only when its prop is; disabled swaps to the muted colour paths.
    let mut header_kids: Vec<UiNode> = Vec::new();
    if let Some(title) = p_text(p, "title") {
        let color = if disabled { "menu.desc" } else { "menu.title" };
        header_kids.push(line(title, p_num(p, "title_size").unwrap_or(20.0), color, "display"));
    }
    if let Some(subtitle) = p_text(p, "subtitle") {
        let color = if disabled { "menu.meta" } else { "menu.caption" };
        header_kids.push(line(subtitle, p_num(p, "subtitle_size").unwrap_or(12.0), color, "label"));
    }

    // Body: the (optional) header column tight over the spliced content slot.
    let mut body = elem("column");
    body.gap = p_num(p, "gap").unwrap_or(10.0) as f32;
    let mut body_kids: Vec<UiNode> = Vec::new();
    if !header_kids.is_empty() {
        let mut header = elem("column");
        header.gap = p_num(p, "header_gap").unwrap_or(4.0) as f32;
        header.children = header_kids;
        body_kids.push(header);
    }
    body_kids.extend(take_slot(slots, "content"));
    body.children = body_kids;

    // Root: the styled slab. The panel style is REUSED by dotted path (default
    // `menu.panel`); `pad` insets the body from the carved-stone edge. Placement
    // (anchor/size/width) is left default so a scene can pin/size the instance
    // through `overlay_placement`.
    let mut panel = with_style(elem("panel"), Some(p_text(p, "style").unwrap_or("menu.panel")));
    panel.pad = p_num(p, "pad").unwrap_or(16.0) as f32;
    panel.children = vec![body];
    panel
}

/// **Popup menu** — the shared modal surface the shell's `MenuView` (menu / pause /
/// confirm, `menu.lua`'s non-launcher path) generalises: a full-bleed `overlay`
/// scrim with a centred (or left-hero) popup `panel` floating **above** it on its
/// own sub-layer, holding a title, an optional subtitle, an optional divider rule,
/// and the caller's vertical stack of `button`s.
///
/// Slots: `items` (the popup's button column — one `button` node per entry the scene
/// supplies, each carrying its own `label`/`action`/`style`), and `muse` (an optional
/// backdrop `sprite`, spliced behind the popup on the low layer so the popup's panel /
/// text / buttons always read over it). Props (all optional): `layout`
/// (`"center"` | `"left"`, default centred), `title` + `title_size` + `title_color`,
/// `subtitle` + `subtitle_size` + `subtitle_color`, `divider` (bool — draw the rule),
/// the dotted style paths `overlay_style` / `panel_style` / `divider_style`, and the
/// `panel_w` / `panel_pad` / `panel_gap` / `items_gap` geometry. Numeric + style
/// defaults mirror the `modal.*` / `screens.*` blocks so the template renders
/// standalone; a scene overrides them from its `ui_elements` section. The builder only
/// names dotted `style`/`color` paths — it never touches a colour, so the one palette
/// can't fork.
fn popup_menu(_ctx: &BuildCtx, p: &HashMap<String, Value>, slots: &mut Slots) -> UiNode {
    // Full-bleed dim / gradient behind the popup — the shell's `screens.<screen>` block.
    let mut scrim = with_style(elem("panel"), Some(p_text(p, "overlay_style").unwrap_or("screens.pause")));
    scrim.anchor = Some(UiAnchor::TopLeft);
    scrim = with_num(scrim, "width_frac", 1.0);
    scrim = with_num(scrim, "height_frac", 1.0);

    // Popup column: title · (subtitle) · (divider) · the button stack.
    let mut col: Vec<UiNode> = Vec::new();

    // Title — colour rides a dotted `color` path (text's escape hatch), not `style`;
    // `size` is the row height it occupies in the flow, `text_size` the glyph size
    // (mirrors `menu.lua`'s `line()` helper: `size = text_size + 10`).
    let title_size = p_num(p, "title_size").unwrap_or(52.0);
    let mut title = with_text(elem("text"), "text", p_text(p, "title").unwrap_or(""));
    title = with_text(title, "align", "center");
    title = with_text(title, "font", "display");
    title = with_text(title, "color", p_text(p, "title_color").unwrap_or("modal.title.color"));
    title = with_num(title, "text_size", title_size);
    title.size = Some((title_size + 10.0) as f32);
    col.push(title);

    if let Some(sub) = p_text(p, "subtitle") {
        let sub_size = p_num(p, "subtitle_size").unwrap_or(11.0);
        let mut subtitle = with_text(elem("text"), "text", sub);
        subtitle = with_text(subtitle, "align", "center");
        subtitle = with_text(subtitle, "font", "label");
        subtitle = with_text(subtitle, "color", p_text(p, "subtitle_color").unwrap_or("modal.subtitle.color"));
        subtitle = with_num(subtitle, "text_size", sub_size);
        subtitle.size = Some((sub_size + 10.0) as f32);
        col.push(subtitle);
    }

    if p_bool(p, "divider") {
        // A 1px styled rule (its fill comes from the block's `color` key, e.g. bronze).
        let mut rule = with_style(elem("panel"), Some(p_text(p, "divider_style").unwrap_or("modal.divider")));
        rule.size = Some(1.0);
        col.push(rule);
    }

    // The scene's button stack, wrapped in a column so it keeps its OWN gap, independent
    // of the popup's title/subtitle spacing (mirrors `menu.lua`'s `button_stack`).
    let mut items = elem("column");
    items.gap = p_num(p, "items_gap").unwrap_or(12.0) as f32;
    items.children = take_slot(slots, "items");
    col.push(items);

    // Optional footer line under the buttons (e.g. the pause screen's "ESC TO RETURN").
    if let Some(footer) = p_text(p, "footer") {
        let fsz = p_num(p, "footer_size").unwrap_or(10.0);
        let mut f = with_text(elem("text"), "text", footer);
        f = with_text(f, "align", "center");
        f = with_text(f, "font", "label");
        f = with_text(f, "color", p_text(p, "footer_color").unwrap_or("modal.footer.color"));
        f = with_num(f, "text_size", fsz);
        f.size = Some((fsz + 10.0) as f32);
        col.push(f);
    }

    // The popup: a fixed-width styled panel lifted onto the UI sub-layer, so a Muse
    // spliced below (low layer) can never cover its panel / text / buttons — within one
    // layer a same-layer sprite draws over a panel, so the popup must sit a layer up.
    let mut popup = with_style(elem("panel"), Some(p_text(p, "panel_style").unwrap_or("modal.panel")));
    popup.width = Some(p_num(p, "panel_w").unwrap_or(404.0) as f32);
    popup.pad = p_num(p, "panel_pad").unwrap_or(38.0) as f32;
    popup.gap = p_num(p, "panel_gap").unwrap_or(16.0) as f32;
    popup = with_num(popup, "layer", 1.0);
    popup.children = col;
    // Placement: centred (pause / confirm) or a left-hero with the menu's 150px inset.
    if p_text(p, "layout") == Some("left") {
        popup.anchor = Some(UiAnchor::Left);
        popup.offset = [150.0, 0.0];
    } else {
        popup.anchor = Some(UiAnchor::Center);
    }

    // Root: a full-bleed overlay page — scrim, then the optional Muse (behind), then the
    // popup. Pinned top-left at frac 1.0 so `overlay_placement` can't re-anchor the whole
    // modal from a scene's placement props (the page always fills its handed rect).
    let mut root = elem("page");
    root.anchor = Some(UiAnchor::TopLeft);
    root = with_num(root, "width_frac", 1.0);
    root = with_num(root, "height_frac", 1.0);
    let mut kids = vec![scrim];
    kids.extend(take_slot(slots, "muse"));
    kids.push(popup);
    root.children = kids;
    root
}

/// One centred text line in a dialog's popup column — mirrors the shell modal's
/// `line`: the node `size` is the row height, the `text_size` prop the glyph size,
/// `color` a dotted path, centred, in the named `font`. A `bind` names a live Model
/// key (e.g. the confirm countdown) in place of literal `text`.
fn dialog_line(
    text: Option<&str>,
    bind: Option<&str>,
    text_size: f64,
    color: &str,
    font: &str,
) -> UiNode {
    let mut n = elem("text");
    n.size = Some((text_size + 10.0) as f32);
    n = with_num(n, "text_size", text_size);
    n.props.insert("color".to_string(), Value::Text(color.to_string()));
    n.props.insert("align".to_string(), Value::Text("center".to_string()));
    n.props.insert("font".to_string(), Value::Text(font.to_string()));
    match bind {
        Some(k) => {
            n.props.insert("text_bind".to_string(), Value::Text(k.to_string()));
        }
        None => {
            n.props
                .insert("text".to_string(), Value::Text(text.unwrap_or("").to_string()));
        }
    }
    n
}

/// One dialog action button: a `button` piece whose `action` fires the choice and
/// whose fill / label come from `modal.buttons.variants.<variant>` (primary
/// sapphire / secondary stone / danger rust). `h` is its height in the button column.
fn variant_button(label: &str, action: &str, variant: &str, h: f64, label_size: f64) -> UiNode {
    let path = format!("modal.buttons.variants.{variant}");
    let mut b = with_style(elem("button"), Some(path.as_str()));
    b.id = action.to_string();
    b.action = Some(action.to_string());
    b.size = Some(h as f32);
    b.props.insert("label".to_string(), Value::Text(label.to_string()));
    b = with_num(b, "label_size", label_size);
    b
}

/// **ChoiceDialog** — a centred modal that poses a question and offers a small set
/// of actions. Generalises the shell's display-confirm overlay
/// (`ConfirmDisplayScene`): a full-screen dim `overlay` blocks the scene behind,
/// and a framed popup floats centred with a `title`, an optional static `message`
/// and/or a live `subtitle_bind` line (the confirm countdown), a divider rule, and
/// a vertical stack of action buttons.
///
/// Buttons come from props by default — a `confirm_*` then a `cancel_*` entry, each
/// `{label, action, variant}` where `variant` selects
/// `modal.buttons.variants.<variant>` — so a scene declares the whole dialog as
/// pure data. A `buttons` slot, when the scene supplies one, replaces the
/// prop-built stack outright (ready `button` nodes, any count / order).
///
/// Composes-a-builder in spirit: the framed popup is built here from
/// panel/text/button pieces through the `dialog_line` / `variant_button` helpers —
/// there is no separate `popup_menu` / `window` builder in the registry yet. If one
/// lands, this can delegate the frame to it via a direct same-module fn call and
/// keep the same prop / slot surface.
///
/// Slots: optional `buttons`. Props (all optional): `title` + `title_size` +
/// `title_color`; `message` + `message_size` + `message_color`; `subtitle_bind` +
/// `subtitle_size` + `subtitle_color`; `overlay_style`, `panel_style`,
/// `divider_style`; `panel_w`, `panel_pad`, `gap`; `btn_h`, `btn_gap`,
/// `btn_label_size`; and the per-button
/// `confirm_label/confirm_action/confirm_variant` +
/// `cancel_label/cancel_action/cancel_variant`. The numeric knobs default to the
/// shared `modal.*` metrics, so a scene usually passes only text + actions.
fn choice_dialog(_ctx: &BuildCtx, p: &HashMap<String, Value>, slots: &mut Slots) -> UiNode {
    // Popup column, top to bottom: title · optional message/live line · divider · buttons.
    let mut col: Vec<UiNode> = Vec::new();

    // Title — the question, in the display face. Defaults to the modal title size.
    col.push(dialog_line(
        p_text(p, "title"),
        None,
        p_num(p, "title_size").unwrap_or(52.0),
        p_text(p, "title_color").unwrap_or("modal.title.color"),
        "display",
    ));

    // A static message line (subtitle face) …
    if let Some(message) = p_text(p, "message") {
        col.push(dialog_line(
            Some(message),
            None,
            p_num(p, "message_size").unwrap_or(11.0),
            p_text(p, "message_color").unwrap_or("modal.subtitle.color"),
            "label",
        ));
    }
    // … and/or a live line bound to a Model key (the confirm countdown), in the body face.
    if let Some(bind) = p_text(p, "subtitle_bind") {
        col.push(dialog_line(
            None,
            Some(bind),
            p_num(p, "subtitle_size").unwrap_or(15.0),
            p_text(p, "subtitle_color").unwrap_or("modal.countdown.color"),
            "body",
        ));
    }

    // Divider rule (the bronze hairline the modal chrome uses).
    let mut divider = with_style(
        elem("panel"),
        Some(p_text(p, "divider_style").unwrap_or("modal.divider")),
    );
    divider.size = Some(1.0);
    col.push(divider);

    // Buttons: a `buttons` slot the scene supplies wins outright; otherwise compose
    // the confirm (primary) then cancel (secondary) entries from props.
    let btn_h = p_num(p, "btn_h").unwrap_or(48.0);
    let btn_label = p_num(p, "btn_label_size").unwrap_or(15.0);
    let slot_buttons = take_slot(slots, "buttons");
    let buttons = if !slot_buttons.is_empty() {
        slot_buttons
    } else {
        let mut bs: Vec<UiNode> = Vec::new();
        if let Some(label) = p_text(p, "confirm_label") {
            bs.push(variant_button(
                label,
                p_text(p, "confirm_action").unwrap_or("confirm"),
                p_text(p, "confirm_variant").unwrap_or("primary"),
                btn_h,
                btn_label,
            ));
        }
        if let Some(label) = p_text(p, "cancel_label") {
            bs.push(variant_button(
                label,
                p_text(p, "cancel_action").unwrap_or("cancel"),
                p_text(p, "cancel_variant").unwrap_or("secondary"),
                btn_h,
                btn_label,
            ));
        }
        bs
    };
    let mut button_col = elem("column");
    button_col.gap = p_num(p, "btn_gap").unwrap_or(12.0) as f32;
    button_col.children = buttons;
    col.push(button_col);

    // The framed popup — centred, auto-heights to its column, lifted above the overlay.
    let mut popup = with_style(
        elem("panel"),
        Some(p_text(p, "panel_style").unwrap_or("modal.panel")),
    );
    popup.id = "popup".to_string();
    popup.anchor = Some(UiAnchor::Center);
    popup.width = Some(p_num(p, "panel_w").unwrap_or(404.0) as f32);
    popup.pad = p_num(p, "panel_pad").unwrap_or(38.0) as f32;
    popup.gap = p_num(p, "gap").unwrap_or(16.0) as f32;
    popup = with_num(popup, "layer", 1.0);
    popup.children = col;

    // The full-screen dim behind the popup — a *styled* overlay claims the pointer,
    // so clicks outside the popup don't fall through to the scene it covers. Omit
    // `overlay_style` for a non-blocking dialog.
    let mut overlay = with_style(elem("panel"), p_text(p, "overlay_style"));
    overlay.anchor = Some(UiAnchor::TopLeft);
    overlay = with_num(overlay, "width_frac", 1.0);
    overlay = with_num(overlay, "height_frac", 1.0);

    // Root: a full-screen overlay page holding the dim + the centred popup.
    let mut root = elem("page");
    root.anchor = Some(UiAnchor::TopLeft);
    root = with_num(root, "width_frac", 1.0);
    root = with_num(root, "height_frac", 1.0);
    root.children = vec![overlay, popup];
    root
}

/// One framed RTT panel: a `grow`ing `stage` wearing `style` and keyed by `id`,
/// that renders `source` into its `StageSlot`. An optional `live_bind` (a Model
/// key) gates whether the slot draws a fresh target this frame. Shared by both
/// sides of `side_by_side_rtt` so the two panels differ only in id / source /
/// liveness. `source` / `live_bind` are omitted when absent, so the walker's own
/// defaults apply (a missing `source` skips the slot with a warn; missing
/// `live_bind` falls through to live).
fn rtt_stage(id: String, source: Option<&str>, style: &str, live_bind: Option<&str>) -> UiNode {
    let mut stage = with_style(elem("stage"), Some(style));
    stage.id = id;
    stage.grow = Some(1.0);
    if let Some(src) = source {
        stage
            .props
            .insert("source".to_string(), Value::Text(src.to_string()));
    }
    if let Some(bind) = live_bind {
        stage
            .props
            .insert("live_bind".to_string(), Value::Text(bind.to_string()));
    }
    stage
}

/// **SideBySideRTT** — a two-panel render-to-texture comparison (In-Place vs
/// Root-Motion, before/after, A/B). A `grow`ing `row` holds two framed `stage`
/// viewports side by side; each stage reserves its own `StageSlot` (keyed by a
/// prefix-derived id) for the scene's frame graph to fill. The left panel renders
/// `left_source`, the right renders `right_source`; both wear the same framed-holder
/// `style`. A per-side `live_bind` (a Model key) gates whether that panel redraws a
/// fresh target this frame or blits its cached poster.
///
/// This is the `SideBySideRTT` the `workbench` doc names for its `viewport` slot:
/// its grow-1 root fills the body row's flexible width when dropped there.
///
/// Props (all optional): `left_source` / `right_source` (the `stages.<source>`
/// sub-scene each side draws), `style` (framed-holder style path, default
/// `assetpipeline.holder`), `gap` (px between the two panels), `left_live_bind` /
/// `right_live_bind` (per-side liveness Model keys). No slots — a stage is a
/// reserved rect, not authored children.
fn side_by_side_rtt(ctx: &BuildCtx, p: &HashMap<String, Value>, _slots: &mut Slots) -> UiNode {
    let style = p_text(p, "style").unwrap_or("assetpipeline.holder");

    let left = rtt_stage(
        format!("{}_left", ctx.id_prefix),
        p_text(p, "left_source"),
        style,
        p_text(p, "left_live_bind"),
    );
    let right = rtt_stage(
        format!("{}_right", ctx.id_prefix),
        p_text(p, "right_source"),
        style,
        p_text(p, "right_live_bind"),
    );

    let mut row = elem("row");
    row.grow = Some(1.0);
    row.gap = p_num(p, "gap").unwrap_or(0.0) as f32;
    row.children = vec![left, right];
    row
}

/// **QuadRTTView** — a thin, framed RTT holder: ONE styled `stage` piece whose
/// reserved [`StageSlot`](crate::component::StageSlot) rect a scene tiles a
/// `QuadGrid` (a 2×2 of render targets) into. Generalises the Kilnworks
/// `editor_quad`: the builder only *places and frames* the slot; the scene fills
/// it from `UiFrame::stages` — so this template takes **no slots**, only scalar
/// props.
///
/// Props (all optional): `source` (the `stages.<source>` offscreen sub-scene to
/// render), `style` (dotted panel style for the frame + its `inset`/`tint`,
/// default `assetpipeline.holder`), `quad_id` (the slot id a scene keys its
/// render target off — else the node's own id via `ctx.id_prefix`), `width` /
/// `height` (a fixed size; otherwise the holder GROWS to fill its flow slot), and
/// the pass-throughs `live` / `live_bind` / `tint` that the walker's stage pass
/// reads straight off the node (see [`StageSlot`](crate::component::StageSlot)).
fn quad_rtt_view(ctx: &BuildCtx, p: &HashMap<String, Value>, _slots: &mut Slots) -> UiNode {
    // The frame is one `stage` piece: the walker draws its dotted panel `style` as
    // the PiP backdrop (and reads `inset` / `tint` from that same block), then
    // reserves the inset rect as a `StageSlot` for the scene to blit into.
    let style = p_text(p, "style").unwrap_or("assetpipeline.holder");
    let mut stage = with_style(elem("stage"), Some(style));

    // Which offscreen sub-scene renders into the slot. The walker skips a stage
    // with no `source`, so a scene supplies it; the builder just forwards it.
    if let Some(source) = p_text(p, "source") {
        stage
            .props
            .insert("source".to_string(), Value::Text(source.to_string()));
    }

    // Stable slot id: an explicit `quad_id` wins, else the template node's own id
    // (`ctx.id_prefix`). A scene keys its per-slot render target off this id.
    stage.id = p_text(p, "quad_id")
        .map(str::to_string)
        .unwrap_or_else(|| ctx.id_prefix.clone());

    // Size: honour a fixed `width` / `height` if asked; otherwise grow to fill the
    // parent's flow slot (the viewport-sized default when dropped into a
    // `workbench` `viewport` slot). `grow` and a fixed size are mutually exclusive
    // in layout, so only set `grow` when neither dimension was pinned.
    let w = p_num(p, "width").map(|v| v as f32);
    let h = p_num(p, "height").map(|v| v as f32);
    stage.width = w;
    stage.height = h;
    if w.is_none() && h.is_none() {
        stage.grow = Some(1.0);
    }

    // Liveness + composite tint ride verbatim through to the walker's stage pass:
    // `live_bind` (a Model key), `live` (a literal bool), `tint` (a dotted colour
    // path). Copy whichever the scene set, preserving its `Value` type.
    for key in ["live", "live_bind", "tint"] {
        if let Some(v) = p.get(key) {
            stage.props.insert(key.to_string(), v.clone());
        }
    }

    stage
}

#[cfg(test)]
mod tests {
    use super::*;

    fn leaf(component: &str) -> UiNode {
        elem(component)
    }

    fn container(component: &str, children: Vec<UiNode>) -> UiNode {
        let mut n = elem(component);
        n.children = children;
        n
    }

    /// A template node: `template` set, `slots` filled.
    fn template_node(name: &str, slots: Vec<(&str, Vec<UiNode>)>) -> UiNode {
        let mut n = elem("template"); // component is ignored when `template` is set
        n.component = String::new();
        n.template = Some(name.to_string());
        n.slots = slots
            .into_iter()
            .map(|(k, v)| (k.to_string(), v))
            .collect();
        n
    }

    #[test]
    fn expand_is_identity_for_template_free_tree() {
        let reg = builtin_templates();
        let tree = container("column", vec![leaf("text"), leaf("button")]);
        // No `template` anywhere → expand must return an equal tree.
        assert_eq!(expand(tree.clone(), &reg), tree);
    }

    #[test]
    fn expand_runs_the_builder_and_splices_slots() {
        let reg = builtin_templates();
        let node = template_node(
            "workbench",
            vec![
                ("header", vec![leaf("text")]),
                ("tabs", vec![leaf("button"), leaf("button")]),
                ("viewport", vec![leaf("stage")]),
                ("rail", vec![leaf("panel")]),
                ("footer", vec![leaf("button")]),
            ],
        );
        let out = expand(node, &reg);
        // Root is the full-screen column with 4 rows: header, tabs, body, footer.
        assert_eq!(out.component, "column");
        assert_eq!(out.children.len(), 4);
        let (header, tabs, body, footer) =
            (&out.children[0], &out.children[1], &out.children[2], &out.children[3]);
        // header row carries its one slot child.
        assert_eq!(header.component, "row");
        assert_eq!(header.children.len(), 1);
        assert_eq!(header.children[0].component, "text");
        // tabs row carries both buttons.
        assert_eq!(tabs.children.len(), 2);
        // body row = viewport ++ rail, in order.
        assert_eq!(body.children.len(), 2);
        assert_eq!(body.children[0].component, "stage");
        assert_eq!(body.children[1].component, "panel");
        assert_eq!(body.grow, Some(1.0));
        // footer = panel > row > [button].
        assert_eq!(footer.component, "panel");
        assert_eq!(footer.children[0].component, "row");
        assert_eq!(footer.children[0].children[0].component, "button");
    }

    #[test]
    fn expand_resolves_a_template_nested_in_a_slot() {
        // A workbench whose `viewport` slot is ITSELF a workbench — post-order
        // expansion must resolve the inner one too (no `template` left in the tree).
        let reg = builtin_templates();
        let inner = template_node("workbench", vec![("header", vec![leaf("text")])]);
        let outer = template_node("workbench", vec![("viewport", vec![inner])]);
        let out = expand(outer, &reg);
        fn has_template(n: &UiNode) -> bool {
            n.template.is_some()
                || n.children.iter().any(has_template)
                || n.slots.values().flatten().any(has_template)
        }
        assert!(!has_template(&out), "no template marker should survive expand");
        // The inner workbench became a column nested in the outer body row.
        assert_eq!(out.children[2].children[0].component, "column");
    }

    #[test]
    fn unknown_template_falls_back_to_an_empty_page() {
        let reg = builtin_templates();
        let mut node = template_node("does_not_exist", vec![]);
        node.id = "keepme".to_string();
        let out = expand(node, &reg);
        assert_eq!(out.component, "page");
        assert_eq!(out.id, "keepme");
        assert!(out.children.is_empty());
    }

    #[test]
    fn overlay_places_the_instance_when_the_builder_is_neutral() {
        // A builder that returns a bare panel; the scene pins it center.
        fn bare(_c: &BuildCtx, _p: &HashMap<String, Value>, _s: &mut Slots) -> UiNode {
            elem("panel")
        }
        let mut reg: TemplateRegistry = HashMap::new();
        reg.insert("bare", bare as TemplateFn);
        let mut node = elem("");
        node.template = Some("bare".to_string());
        node.anchor = Some(UiAnchor::Center);
        node.width = Some(400.0);
        let out = expand(node, &reg);
        assert_eq!(out.component, "panel");
        assert_eq!(out.anchor, Some(UiAnchor::Center));
        assert_eq!(out.width, Some(400.0));
    }

    #[test]
    fn window_frames_titlebar_body_footer_and_rune_overlay() {
        let reg = builtin_templates();
        let mut node = template_node(
            "window",
            vec![
                ("content", vec![leaf("stage")]),
                ("footer", vec![leaf("button")]),
            ],
        );
        node.props.insert("title".to_string(), Value::Text("Grimoire".to_string()));
        node.props.insert("w".to_string(), Value::Number(640.0));
        node.props.insert("h".to_string(), Value::Number(480.0));
        node.props.insert("has_min".to_string(), Value::Bool(true));
        let out = expand(node, &reg);

        // Root = the styled frame STACK, sized + centred, overlaying [chrome, runes].
        assert_eq!(out.component, "stack");
        assert_eq!(p_text(&out.props, "style"), Some("settings.window"));
        assert_eq!(out.anchor, Some(UiAnchor::Center));
        assert_eq!(out.width, Some(640.0));
        assert_eq!(out.height, Some(480.0));
        assert_eq!(out.children.len(), 2);

        let (col, runes) = (&out.children[0], &out.children[1]);
        // Overlay: rune_corners fills the frame and points at the reused runes block.
        assert_eq!(runes.component, "rune_corners");
        assert_eq!(p_text(&runes.props, "style"), Some("settings.runes"));

        // Chrome column = title bar · content well · footer (footer slot non-empty).
        assert_eq!(col.component, "column");
        assert_eq!(col.children.len(), 3);
        let (titlebar, well, footer) = (&col.children[0], &col.children[1], &col.children[2]);

        // Title bar: text title · grow spacer · minimize · close (has_min = true).
        assert_eq!(titlebar.component, "row");
        assert_eq!(p_text(&titlebar.props, "style"), Some("settings.titlebar"));
        assert_eq!(titlebar.children.len(), 4);
        assert_eq!(titlebar.children[0].component, "text");
        assert_eq!(p_text(&titlebar.children[0].props, "text"), Some("Grimoire"));
        assert_eq!(titlebar.children[1].component, "stack");
        assert_eq!(titlebar.children[1].grow, Some(1.0));
        let (min_btn, close_btn) = (&titlebar.children[2], &titlebar.children[3]);
        assert_eq!(min_btn.component, "button");
        assert_eq!(min_btn.action.as_deref(), Some("minimize"));
        assert_eq!(close_btn.component, "button");
        assert_eq!(close_btn.action.as_deref(), Some("close")); // default close_action
        assert_eq!(p_text(&close_btn.props, "style"), Some("settings.titlebar.close"));

        // Content well: a growing panel wrapping the `content` slot verbatim.
        assert_eq!(well.component, "panel");
        assert_eq!(well.grow, Some(1.0));
        assert_eq!(well.children.len(), 1);
        assert_eq!(well.children[0].component, "stage");

        // Footer: a panel > row > [the footer slot button] (mirrors workbench).
        assert_eq!(footer.component, "panel");
        assert_eq!(footer.children[0].component, "row");
        assert_eq!(footer.children[0].children[0].component, "button");
    }

    #[test]
    fn window_omits_the_footer_bar_when_the_slot_is_empty() {
        let reg = builtin_templates();
        // Only a content slot; no footer, no has_min.
        let node = template_node("window", vec![("content", vec![leaf("text")])]);
        let out = expand(node, &reg);
        let col = &out.children[0];
        // Just the title bar + the content well — no footer panel appended.
        assert_eq!(col.children.len(), 2);
        assert_eq!(col.children[0].component, "row"); // title bar
        assert_eq!(col.children[1].component, "panel"); // content well
        // Close shows by default; minimize does not (has_min unset) → title · spacer · close.
        let titlebar = &col.children[0];
        assert_eq!(titlebar.children.len(), 3);
        assert_eq!(titlebar.children.last().unwrap().action.as_deref(), Some("close"));
    }

    #[test]
    fn card_wraps_a_styled_panel_around_header_and_content() {
        let reg = builtin_templates();
        let mut node = elem("");
        node.template = Some("card".to_string());
        node.props
            .insert("title".to_string(), Value::Text("Chest Piece".to_string()));
        node.props
            .insert("subtitle".to_string(), Value::Text("PLATE · TIER II".to_string()));
        node.slots
            .insert("content".to_string(), vec![leaf("text"), leaf("button")]);
        let out = expand(node, &reg);

        // Root: a carved-stone panel REUSING the menu.panel style block.
        assert_eq!(out.component, "panel");
        assert_eq!(
            out.props.get("style"),
            Some(&Value::Text("menu.panel".to_string()))
        );

        // panel > column(body) > [ header column, ...content ]
        assert_eq!(out.children.len(), 1);
        let body = &out.children[0];
        assert_eq!(body.component, "column");
        assert_eq!(body.children.len(), 3);

        // Header groups the title (display face) over the subtitle (label caps).
        let header = &body.children[0];
        assert_eq!(header.component, "column");
        assert_eq!(header.children.len(), 2);
        let (title, sub) = (&header.children[0], &header.children[1]);
        assert_eq!(title.component, "text");
        assert_eq!(
            title.props.get("text"),
            Some(&Value::Text("Chest Piece".to_string()))
        );
        assert_eq!(
            title.props.get("color"),
            Some(&Value::Text("menu.title".to_string()))
        );
        assert_eq!(
            title.props.get("font"),
            Some(&Value::Text("display".to_string()))
        );
        assert_eq!(
            sub.props.get("color"),
            Some(&Value::Text("menu.caption".to_string()))
        );

        // The content slot is spliced in after the header, in order.
        assert_eq!(body.children[1].component, "text");
        assert_eq!(body.children[2].component, "button");
    }

    #[test]
    fn card_dims_the_header_when_disabled_and_omits_an_absent_header() {
        let reg = builtin_templates();

        // Disabled → title/subtitle swap to muted colour paths (no 2nd style block).
        let mut node = elem("");
        node.template = Some("card".to_string());
        node.props
            .insert("title".to_string(), Value::Text("Locked".to_string()));
        node.props
            .insert("subtitle".to_string(), Value::Text("REQUIRES KEY".to_string()));
        node.props.insert("disabled".to_string(), Value::Bool(true));
        node.slots
            .insert("content".to_string(), vec![leaf("panel")]);
        let out = expand(node, &reg);
        let header = &out.children[0].children[0];
        assert_eq!(
            header.children[0].props.get("color"),
            Some(&Value::Text("menu.desc".to_string()))
        );
        assert_eq!(
            header.children[1].props.get("color"),
            Some(&Value::Text("menu.meta".to_string()))
        );

        // No title and no subtitle → no header column; the body is just the content.
        let mut bare = elem("");
        bare.template = Some("card".to_string());
        bare.slots
            .insert("content".to_string(), vec![leaf("text")]);
        let out2 = expand(bare, &reg);
        let body2 = &out2.children[0];
        assert_eq!(body2.children.len(), 1);
        assert_eq!(body2.children[0].component, "text");
    }

    #[test]
    fn popup_menu_composes_scrim_muse_and_layered_popup() {
        let reg = builtin_templates();
        // A left-hero menu popup: title + subtitle + divider + two launch buttons, a Muse
        // sprite behind, styles pointed at the shell's blocks.
        let mut node = template_node(
            "popup_menu",
            vec![
                ("items", vec![leaf("button"), leaf("button")]),
                ("muse", vec![leaf("sprite")]),
            ],
        );
        node.props.insert("layout".into(), Value::Text("left".into()));
        node.props.insert("title".into(), Value::Text("Prism".into()));
        node.props.insert("title_size".into(), Value::Number(58.0));
        node.props.insert("subtitle".into(), Value::Text("THE SEVEN SHARDS".into()));
        node.props.insert("divider".into(), Value::Bool(true));
        node.props.insert("overlay_style".into(), Value::Text("screens.menu".into()));
        node.props.insert("panel_style".into(), Value::Text("modal.panel".into()));

        let out = expand(node, &reg);

        // Root is a full-bleed overlay page: scrim, muse sprite, popup — in that order.
        assert_eq!(out.component, "page");
        assert_eq!(out.anchor, Some(UiAnchor::TopLeft));
        assert_eq!(out.children.len(), 3);
        let (scrim, muse, popup) = (&out.children[0], &out.children[1], &out.children[2]);

        // Scrim: a full-bleed panel styled by `overlay_style`.
        assert_eq!(scrim.component, "panel");
        assert_eq!(scrim.props.get("style"), Some(&Value::Text("screens.menu".into())));
        assert_eq!(scrim.anchor, Some(UiAnchor::TopLeft));
        // Muse slot spliced verbatim, behind the popup.
        assert_eq!(muse.component, "sprite");

        // Popup: a `modal.panel` lifted onto sub-layer 1 (so the Muse can't cover it),
        // left-anchored with the hero offset.
        assert_eq!(popup.component, "panel");
        assert_eq!(popup.props.get("style"), Some(&Value::Text("modal.panel".into())));
        assert_eq!(popup.props.get("layer"), Some(&Value::Number(1.0)));
        assert_eq!(popup.anchor, Some(UiAnchor::Left));
        assert_eq!(popup.offset, [150.0, 0.0]);

        // Popup column: title · subtitle · divider · items-column(2 buttons).
        assert_eq!(popup.children.len(), 4);
        let (title, subtitle, divider, items) = (
            &popup.children[0],
            &popup.children[1],
            &popup.children[2],
            &popup.children[3],
        );
        assert_eq!(title.component, "text");
        assert_eq!(title.props.get("text"), Some(&Value::Text("Prism".into())));
        assert_eq!(title.props.get("color"), Some(&Value::Text("modal.title.color".into())));
        assert_eq!(title.props.get("font"), Some(&Value::Text("display".into())));
        assert_eq!(subtitle.component, "text");
        assert_eq!(subtitle.props.get("text"), Some(&Value::Text("THE SEVEN SHARDS".into())));
        assert_eq!(subtitle.props.get("color"), Some(&Value::Text("modal.subtitle.color".into())));
        assert_eq!(divider.component, "panel");
        assert_eq!(divider.props.get("style"), Some(&Value::Text("modal.divider".into())));
        assert_eq!(divider.size, Some(1.0));
        // Items: the caller's buttons wrapped in a column so they carry their own gap.
        assert_eq!(items.component, "column");
        assert_eq!(items.children.len(), 2);
        assert_eq!(items.children[0].component, "button");
        assert_eq!(items.children[1].component, "button");
    }

    #[test]
    fn popup_menu_centers_and_omits_optional_pieces() {
        let reg = builtin_templates();
        // A confirm-style popup: centred, no subtitle, no divider, no muse — just a title
        // and one button. The optional pieces must be absent and the popup centred, and the
        // scrim must fall back to the default overlay style.
        let mut node = template_node("popup_menu", vec![("items", vec![leaf("button")])]);
        node.props.insert("title".into(), Value::Text("Keep Display?".into()));

        let out = expand(node, &reg);
        assert_eq!(out.component, "page");
        // Only scrim + popup (no muse spliced).
        assert_eq!(out.children.len(), 2);
        let (scrim, popup) = (&out.children[0], &out.children[1]);
        // Default scrim style when none supplied.
        assert_eq!(scrim.props.get("style"), Some(&Value::Text("screens.pause".into())));
        // Centred, no hero offset.
        assert_eq!(popup.anchor, Some(UiAnchor::Center));
        assert_eq!(popup.offset, [0.0, 0.0]);
        // Column is just [title, items] — subtitle and divider omitted.
        assert_eq!(popup.children.len(), 2);
        assert_eq!(popup.children[0].component, "text");
        assert_eq!(popup.children[1].component, "column");
        assert_eq!(popup.children[1].children.len(), 1);
        assert_eq!(popup.children[1].children[0].component, "button");
    }

    #[test]
    fn choice_dialog_composes_overlay_popup_and_prop_buttons() {
        let reg = builtin_templates();
        let mut node = template_node("choice_dialog", vec![]);
        node.props = [
            ("title", Value::Text("Keep Display?".to_string())),
            ("title_size", Value::Number(38.0)),
            ("overlay_style", Value::Text("screens.confirm".to_string())),
            ("subtitle_bind", Value::Text("subtitle".to_string())),
            ("confirm_label", Value::Text("KEEP".to_string())),
            ("confirm_action", Value::Text("keep".to_string())),
            ("confirm_variant", Value::Text("primary".to_string())),
            ("cancel_label", Value::Text("REVERT".to_string())),
            ("cancel_action", Value::Text("revert".to_string())),
            ("cancel_variant", Value::Text("danger".to_string())),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v))
        .collect();
        let out = expand(node, &reg);

        // Root: a full-screen overlay page = [dim overlay, centred popup].
        assert_eq!(out.component, "page");
        assert_eq!(p_num(&out.props, "width_frac"), Some(1.0));
        assert_eq!(out.children.len(), 2);
        let (overlay, popup) = (&out.children[0], &out.children[1]);

        // Overlay: full-screen panel wearing the scene's overlay style.
        assert_eq!(overlay.component, "panel");
        assert_eq!(p_text(&overlay.props, "style"), Some("screens.confirm"));
        assert_eq!(p_num(&overlay.props, "width_frac"), Some(1.0));

        // Popup: centred modal panel, lifted above the overlay.
        assert_eq!(popup.component, "panel");
        assert_eq!(popup.anchor, Some(UiAnchor::Center));
        assert_eq!(p_text(&popup.props, "style"), Some("modal.panel"));
        assert_eq!(p_num(&popup.props, "layer"), Some(1.0));

        // Column: title · live subtitle · divider · button column.
        assert_eq!(popup.children.len(), 4);
        let (title, subtitle, divider, buttons) = (
            &popup.children[0],
            &popup.children[1],
            &popup.children[2],
            &popup.children[3],
        );
        assert_eq!(title.component, "text");
        assert_eq!(p_text(&title.props, "text"), Some("Keep Display?"));
        assert_eq!(p_num(&title.props, "text_size"), Some(38.0));
        assert_eq!(p_text(&title.props, "align"), Some("center"));
        assert_eq!(subtitle.component, "text");
        assert_eq!(p_text(&subtitle.props, "text_bind"), Some("subtitle"));
        assert_eq!(divider.component, "panel");
        assert_eq!(p_text(&divider.props, "style"), Some("modal.divider"));

        // Two prop-built buttons, in order, each variant-styled with its action.
        assert_eq!(buttons.component, "column");
        assert_eq!(buttons.children.len(), 2);
        let (keep, revert) = (&buttons.children[0], &buttons.children[1]);
        assert_eq!(keep.component, "button");
        assert_eq!(keep.action.as_deref(), Some("keep"));
        assert_eq!(p_text(&keep.props, "label"), Some("KEEP"));
        assert_eq!(
            p_text(&keep.props, "style"),
            Some("modal.buttons.variants.primary")
        );
        assert_eq!(revert.action.as_deref(), Some("revert"));
        assert_eq!(
            p_text(&revert.props, "style"),
            Some("modal.buttons.variants.danger")
        );
    }

    #[test]
    fn choice_dialog_buttons_slot_overrides_prop_buttons() {
        let reg = builtin_templates();
        let mut node = template_node(
            "choice_dialog",
            vec![("buttons", vec![leaf("button"), leaf("button"), leaf("button")])],
        );
        node.props = [
            ("title", Value::Text("Delete this clip?".to_string())),
            ("confirm_label", Value::Text("YES".to_string())),
            ("cancel_label", Value::Text("NO".to_string())),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v))
        .collect();
        let out = expand(node, &reg);

        // No message / subtitle → popup column is [title, divider, buttons].
        let popup = &out.children[1];
        let buttons = popup.children.last().expect("button column present");
        assert_eq!(buttons.component, "column");
        // The slot's three buttons win; the confirm/cancel props are ignored.
        assert_eq!(buttons.children.len(), 3);
        assert!(buttons.children.iter().all(|b| b.component == "button"));
    }

    #[test]
    fn side_by_side_rtt_emits_two_styled_stages_in_a_row() {
        let reg = builtin_templates();
        // A template node with the two sources, a gap, and a left-only live bind.
        let mut node = template_node("side_by_side_rtt", vec![]);
        node.id = "cmp".to_string();
        node.props
            .insert("left_source".to_string(), Value::Text("inplace".to_string()));
        node.props
            .insert("right_source".to_string(), Value::Text("rootmotion".to_string()));
        node.props.insert("gap".to_string(), Value::Number(12.0));
        node.props
            .insert("left_live_bind".to_string(), Value::Text("left_live".to_string()));
        let out = expand(node, &reg);

        // Root: a growing row carrying exactly the two panels, gap from the prop.
        assert_eq!(out.component, "row");
        assert_eq!(out.grow, Some(1.0));
        assert_eq!(out.gap, 12.0);
        assert_eq!(out.children.len(), 2);

        let (left, right) = (&out.children[0], &out.children[1]);
        // Both are styled `stage`s, each grown to share the row, ids from the prefix.
        assert_eq!(left.component, "stage");
        assert_eq!(right.component, "stage");
        assert_eq!(left.grow, Some(1.0));
        assert_eq!(right.grow, Some(1.0));
        assert_eq!(left.id, "cmp_left");
        assert_eq!(right.id, "cmp_right");
        // Each side's `source` routed to the matching stage.
        assert_eq!(p_text(&left.props, "source"), Some("inplace"));
        assert_eq!(p_text(&right.props, "source"), Some("rootmotion"));
        // Default framed-holder style on both (no `style` prop supplied).
        assert_eq!(p_text(&left.props, "style"), Some("assetpipeline.holder"));
        assert_eq!(p_text(&right.props, "style"), Some("assetpipeline.holder"));
        // Optional per-side `live_bind`: present on the left, absent on the right.
        assert_eq!(p_text(&left.props, "live_bind"), Some("left_live"));
        assert!(!right.props.contains_key("live_bind"));
    }

    /// Read a `Text` prop off a built node (the tests-module twin of the walker's
    /// private `ptext`, which is not visible here).
    fn text_prop<'a>(n: &'a UiNode, key: &str) -> Option<&'a str> {
        match n.props.get(key) {
            Some(Value::Text(t)) => Some(t.as_str()),
            _ => None,
        }
    }

    /// A `quad_rtt_view` with only a `source` expands to ONE styled `stage` piece
    /// that grows to fill its slot, defaults to the shared `assetpipeline.holder`
    /// frame, defaults its slot id to the node's own id, and forwards `source` +
    /// the `live_bind` / `tint` pass-throughs for the walker's stage pass.
    #[test]
    fn quad_rtt_view_frames_a_growing_stage_slot() {
        let reg = builtin_templates();
        let mut node = template_node("quad_rtt_view", vec![]);
        node.id = "editor_quad".to_string();
        node.props
            .insert("source".to_string(), Value::Text("turntable".to_string()));
        node.props
            .insert("live_bind".to_string(), Value::Text("quad_live".to_string()));
        node.props.insert(
            "tint".to_string(),
            Value::Text("assetpipeline.holder.tint".to_string()),
        );
        let out = expand(node, &reg);

        // One `stage` piece — a thin holder, no children and no leftover slots.
        assert_eq!(out.component, "stage");
        assert!(out.children.is_empty());
        assert!(out.slots.is_empty());
        // Default frame style + forwarded source / liveness / tint props.
        assert_eq!(text_prop(&out, "style"), Some("assetpipeline.holder"));
        assert_eq!(text_prop(&out, "source"), Some("turntable"));
        assert_eq!(text_prop(&out, "live_bind"), Some("quad_live"));
        assert_eq!(text_prop(&out, "tint"), Some("assetpipeline.holder.tint"));
        // Slot id defaults to the node's own id; the holder grows to fill its slot.
        assert_eq!(out.id, "editor_quad");
        assert_eq!(out.grow, Some(1.0));
        assert_eq!(out.width, None);
        assert_eq!(out.height, None);
    }

    /// The overrides: an explicit `quad_id` becomes the slot id, a `style` prop
    /// replaces the default frame, a fixed `width`/`height` suppresses `grow` (the
    /// two are mutually exclusive in layout), and a literal `live` bool rides
    /// through verbatim.
    #[test]
    fn quad_rtt_view_honours_quad_id_style_and_fixed_size() {
        let reg = builtin_templates();
        let mut node = template_node("quad_rtt_view", vec![]);
        node.id = "node_id".to_string();
        node.props
            .insert("quad_id".to_string(), Value::Text("kiln_grid".to_string()));
        node.props.insert(
            "style".to_string(),
            Value::Text("loomforge.clip_stage".to_string()),
        );
        node.props
            .insert("source".to_string(), Value::Text("lighting".to_string()));
        node.props.insert("width".to_string(), Value::Number(512.0));
        node.props.insert("height".to_string(), Value::Number(512.0));
        node.props.insert("live".to_string(), Value::Bool(false));
        let out = expand(node, &reg);

        assert_eq!(out.component, "stage");
        // `quad_id` wins over the node id; the explicit style overrides the default.
        assert_eq!(out.id, "kiln_grid");
        assert_eq!(text_prop(&out, "style"), Some("loomforge.clip_stage"));
        assert_eq!(text_prop(&out, "source"), Some("lighting"));
        // A fixed size means NO grow, so it lays out at the requested pixels.
        assert_eq!(out.width, Some(512.0));
        assert_eq!(out.height, Some(512.0));
        assert_eq!(out.grow, None);
        // Literal liveness bool rides through as a `Bool` value.
        assert_eq!(out.props.get("live"), Some(&Value::Bool(false)));
    }
}
