//! The **template tier** — the middle of `Scene → Template → Element`.
//!
//! A *piece* is one leaf `component` the walker draws (`button`, `slider`, …). A
//! *template* composes pieces into a reusable arrangement (a Workbench, a
//! PopupMenu, a ChoiceDialog). A *scene* declares a tree of placed nodes — some
//! plain pieces, some `template` references with `slots` filled — as DATA;
//! [`expand`] turns every `template` node into the subtree its definition
//! produces, once, before the tree is cached and walked.
//!
//! A template is **pure data by default**: a proto `UiNode` tree in the embedded
//! `ui_templates.json` (a [`TemplateDef::Data`] entry), instantiated by
//! substituting the instance's props (`@name` / `@{name}` / `when` — see
//! `substitute`) and splicing its slots (see `splice_slots`). Only the
//! STRUCTURAL Components stay **Rust builders** ([`TemplateDef::Builder`]):
//! `frame` (the 3×3 border grid needs computed track strings), `card`, and
//! `option_grid` (row-chunking) — and a data proto composes them by reference
//! (`window` is a `{ "template": "frame" }` node in data).
//!
//! This crate owns the tier because instantiation emits [`UiNode`]s — the widget
//! layer's job. `flicker-script` only *parses* the tree (it gained `template` +
//! `slots` as plain-data fields); it never learns what a template is, so the
//! crate stack stays `flicker-widgets → flicker-script`, never the reverse.
//!
//! Builders and protos are **pure**: they read scalar props + move named slots
//! into place and emit nodes whose `style` prop names a dotted path into
//! `theme.tokens`. They never touch a colour, so a template cannot fork the one
//! palette. `$token` strings pass through untouched — the stringtable resolves
//! them at draw.

use std::collections::HashMap;
use std::sync::LazyLock;

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

/// One registered template: either a Rust **builder** fn (the structural
/// Components — `frame` / `card` / `option_grid` — and any bespoke builder a scene
/// crate registers) or a **data proto** — a `UiNode` tree as plain JSON that
/// [`expand`] instantiates by substituting the instance's props and splicing its
/// slots. Data protos live in `ui_templates.json`; the `&'static` borrow points
/// into the embedded, parse-once [`TEMPLATE_DATA`] document.
pub enum TemplateDef {
    Builder(TemplateFn),
    Data(&'static serde_json::Value),
}

/// The name → template table. A flat `HashMap` is the lightest thing that is still
/// a registry: [`builtin_templates`] is the one place a template is registered,
/// and a scene crate can `.insert()` a bespoke [`TemplateDef::Builder`] before it
/// calls [`expand`] — without editing this crate.
pub type TemplateRegistry = HashMap<String, TemplateDef>;

/// The embedded data-template protos (`ui_templates.json`), parsed ONCE. The file
/// rides `include_str!` exactly like the `ui/*.lua` component modules in `lib.rs`,
/// so a client binary needs no content path to resolve the built-in templates. A
/// parse failure warns and yields an empty `templates` map (best-effort, like every
/// loader here) — the affected templates then fail visibly as "unknown template".
static TEMPLATE_DATA: LazyLock<serde_json::Value> = LazyLock::new(|| {
    serde_json::from_str(include_str!(
        "../../../../content/sensorium/resources/ui_templates.json"
    ))
    .unwrap_or_else(|e| {
        tracing::error!("ui_templates.json parse failed: {e}");
        serde_json::json!({ "templates": {} })
    })
});

/// The built-in template set: every key of the embedded `ui_templates.json` as a
/// [`TemplateDef::Data`] proto, plus the surviving Rust builders. Builders are
/// inserted second so a template mid-migration (a proto authored while its builder
/// still exists) keeps resolving to the builder until the builder is deleted —
/// the flip IS the deletion.
pub fn builtin_templates() -> TemplateRegistry {
    let mut m: TemplateRegistry = HashMap::new();
    if let Some(templates) = TEMPLATE_DATA.get("templates").and_then(serde_json::Value::as_object) {
        for (name, proto) in templates {
            m.insert(name.clone(), TemplateDef::Data(proto));
        }
    }
    for (name, f) in [
        ("frame", frame as TemplateFn),
        ("card", card as TemplateFn),
        ("option_grid", option_grid as TemplateFn),
    ] {
        m.insert(name.to_string(), TemplateDef::Builder(f));
    }
    m
}

/// The template-expansion nesting bound: a chain of template resolutions (a data
/// proto referencing another template, which references another, …) deeper than
/// this warns and falls back to the empty screen — the guard against a proto that
/// (transitively) references itself. Ordinary tree depth is NOT counted; only a
/// template resolving from within another template's output goes one level down.
pub const MAX_TEMPLATE_DEPTH: usize = 8;

/// Expand every `template` node into the subtree its builder produces (or its data
/// proto instantiates), depth-first (post-order): a node's `children` and `slots`
/// are expanded **before** the node itself, so a template that emits another
/// template still resolves. A template-free tree is returned UNCHANGED (structural
/// identity) — the reason a scene not yet using templates is completely unaffected
/// when `expand` is wired into its load path.
pub fn expand(node: UiNode, reg: &TemplateRegistry) -> UiNode {
    expand_depth(node, reg, 0)
}

/// [`expand`] with the template-nesting depth threaded through: children/slots
/// recurse at the SAME depth (tree depth is free); resolving a template hands its
/// built subtree to `depth + 1`, so only template-within-template chains count
/// toward [`MAX_TEMPLATE_DEPTH`].
fn expand_depth(mut node: UiNode, reg: &TemplateRegistry, depth: usize) -> UiNode {
    node.children = node.children.into_iter().map(|c| expand_depth(c, reg, depth)).collect();
    let mut slots: Slots = std::mem::take(&mut node.slots);
    for group in slots.values_mut() {
        let done: Vec<UiNode> = std::mem::take(group)
            .into_iter()
            .map(|c| expand_depth(c, reg, depth))
            .collect();
        *group = done;
    }

    let Some(name) = node.template.take() else {
        // An ordinary piece/container — returned verbatim.
        node.slots = slots;
        return node;
    };

    if depth >= MAX_TEMPLATE_DEPTH {
        // A proto chain this deep is a cycle in practice; stand in the empty page
        // (the same fallback as an unknown name) so it fails visibly, not by hang.
        tracing::warn!(
            "ui arrangement: template `{name}` exceeded the max expansion depth ({MAX_TEMPLATE_DEPTH})"
        );
        return empty_screen(node);
    }

    match reg.get(name.as_str()) {
        Some(TemplateDef::Builder(builder)) => {
            let ctx = BuildCtx {
                id_prefix: node.id.clone(),
            };
            let mut built = builder(&ctx, &node.props, &mut slots);
            overlay_placement(&mut built, &node);
            built
        }
        Some(TemplateDef::Data(proto)) => {
            let props = subst_props(&node);
            let json = match substitute(proto, &props) {
                Some(json) => json,
                None => {
                    tracing::warn!("ui arrangement: template `{name}` proto substituted away");
                    return empty_screen(node);
                }
            };
            let parsed = match flicker_script::parse_ui_json(&json) {
                Ok(parsed) => parsed,
                Err(e) => {
                    tracing::warn!("ui arrangement: template `{name}` proto did not parse: {e}");
                    return empty_screen(node);
                }
            };
            let (mut roots, _) = splice_slots(parsed, &mut slots);
            if roots.len() != 1 {
                tracing::warn!(
                    "ui arrangement: template `{name}` resolved to {} roots (want exactly 1)",
                    roots.len()
                );
                return empty_screen(node);
            }
            // Resolve templates the proto itself references (`frame`, …) one nesting
            // level down, then let the instance place the finished root — exactly the
            // Builder arm's tail.
            let mut built = expand_depth(roots.remove(0), reg, depth + 1);
            overlay_placement(&mut built, &node);
            built
        }
        None => {
            // Best-effort, like every other loader here: warn and stand in an empty
            // page so a typo'd template name fails visibly, not with a panic.
            tracing::warn!("ui arrangement: unknown template `{name}`");
            empty_screen(node)
        }
    }
}

/// The failed-template stand-in: an empty `screen` carrying the instance node's
/// own placement, so a bad template fails visibly in place, never with a panic.
fn empty_screen(node: UiNode) -> UiNode {
    UiNode {
        component: "screen".to_string(),
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

// ── the data-proto instantiation pipeline (the `TemplateDef::Data` arm) ─────────
//
// A proto is a `UiNode` tree as plain JSON. Instantiation is three passes:
//   1. `substitute` — instance props into the JSON (`@name` exact / `@{name}`
//      interpolation / `when` gates), BEFORE any parse;
//   2. `flicker_script::parse_ui_json` — the ONE arrangement reader;
//   3. `splice_slots` — `slot` nodes replaced by the instance's slot content
//      (post-parse, so spliced nodes are real `UiNode`s, already expanded).
// Then the result is `expand_depth`ed (protos may reference other templates) and
// `overlay_placement`d like any built subtree.

/// The substitution context an instance hands its proto: the node's own `props`
/// plus the STRUCTURAL pseudo-props — `anchor` (the parsers consume an instance's
/// `anchor` key structurally, so a proto could never see it as a prop) and `id`
/// (the data twin of `BuildCtx::id_prefix`, always present, possibly empty). A
/// real prop with either name wins over the pseudo-prop.
fn subst_props(node: &UiNode) -> HashMap<String, Value> {
    let mut ctx = node.props.clone();
    if let Some(a) = node.anchor {
        ctx.entry("anchor".to_string())
            .or_insert_with(|| Value::Text(anchor_name(a).to_string()));
    }
    ctx.entry("id".to_string()).or_insert_with(|| Value::Text(node.id.clone()));
    ctx
}

/// The inverse of `flicker_script`'s anchor parse — the pseudo-prop rendering of
/// an instance's structural `anchor` for `@anchor` substitution.
fn anchor_name(a: UiAnchor) -> &'static str {
    match a {
        UiAnchor::TopLeft => "top_left",
        UiAnchor::Top => "top",
        UiAnchor::TopRight => "top_right",
        UiAnchor::Left => "left",
        UiAnchor::Center => "center",
        UiAnchor::Right => "right",
        UiAnchor::BottomLeft => "bottom_left",
        UiAnchor::Bottom => "bottom",
        UiAnchor::BottomRight => "bottom_right",
    }
}

/// Substitute instance `props` into a proto JSON value, recursively. Returns
/// `None` when the VALUE resolves to "absent" — its holder removes the object key
/// / drops the array element (the data twin of a builder's `if let Some(v) =
/// p_text(..)` arms). The rules, applied JSON-level BEFORE any parse:
///
/// * a string that is EXACTLY `@name` / `@name=default` → the prop's NATIVE JSON
///   value (bool / number / string); absent → the default (`true`/`false` → bool,
///   numeric-looking → number, else string; `=` alone → empty string); absent with
///   no default → removal;
/// * a string CONTAINING `@{name}` / `@{name=default}` → string interpolation
///   (each occurrence replaced by the prop's text / number / bool rendering;
///   an absent prop with no default removes the WHOLE value);
/// * an object carrying `"when": "@name"` (truthy: present, not `false`, not
///   empty text) or `"when": "!@name"` (negated) → dropped when the condition
///   fails, `when` key stripped when it passes;
/// * `$`-prefixed strings (`$token` stringtable refs, `$$` escapes) pass through
///   UNTOUCHED — they resolve at draw, never here.
fn substitute(proto: &serde_json::Value, props: &HashMap<String, Value>) -> Option<serde_json::Value> {
    use serde_json::Value as J;
    match proto {
        J::String(s) => subst_string(s, props),
        J::Array(items) => Some(J::Array(items.iter().filter_map(|v| substitute(v, props)).collect())),
        J::Object(map) => {
            if let Some(when) = map.get("when") {
                match when {
                    J::String(cond) if !when_passes(cond, props) => return None,
                    J::String(_) => {}
                    other => tracing::warn!("ui template proto: non-string `when` ({other}) ignored"),
                }
            }
            let mut out = serde_json::Map::new();
            for (k, v) in map {
                if k == "when" {
                    continue;
                }
                if let Some(sv) = substitute(v, props) {
                    out.insert(k.clone(), sv);
                }
            }
            Some(J::Object(out))
        }
        other => Some(other.clone()),
    }
}

/// A prop is TRUTHY for a `when` gate when it is present, not `Bool(false)`, and
/// not empty text. (Numbers — including 0 — are truthy: presence is the signal.)
fn when_passes(cond: &str, props: &HashMap<String, Value>) -> bool {
    let (negated, name) = match cond.strip_prefix("!@") {
        Some(name) => (true, name),
        None => match cond.strip_prefix('@') {
            Some(name) => (false, name),
            None => {
                tracing::warn!("ui template proto: malformed `when` condition `{cond}` ignored");
                return true;
            }
        },
    };
    let truthy = match props.get(name) {
        None | Some(Value::Bool(false)) => false,
        Some(Value::Text(t)) => !t.is_empty(),
        Some(_) => true,
    };
    negated != truthy
}

/// Substitute ONE proto string per the rules on [`substitute`].
fn subst_string(s: &str, props: &HashMap<String, Value>) -> Option<serde_json::Value> {
    use serde_json::Value as J;
    // `$token` / `$$` stringtable refs resolve at draw — never substituted here.
    if s.starts_with('$') {
        return Some(J::String(s.to_string()));
    }
    // Exact form: `@name` or `@name=default` (and nothing else in the string).
    if let Some(rest) = s.strip_prefix('@') {
        if !rest.starts_with('{') {
            let name_len = rest.chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '_').count();
            let (name, tail) = rest.split_at(name_len);
            if !name.is_empty() && (tail.is_empty() || tail.starts_with('=')) {
                return match props.get(name) {
                    Some(Value::Bool(b)) => Some(J::Bool(*b)),
                    Some(Value::Number(n)) => match serde_json::Number::from_f64(*n) {
                        Some(n) => Some(J::Number(n)),
                        None => {
                            tracing::warn!("ui template proto: non-finite prop `{name}` removed");
                            None
                        }
                    },
                    Some(Value::Text(t)) => Some(J::String(t.clone())),
                    None => tail.strip_prefix('=').map(parse_default),
                };
            }
        }
    }
    // Interpolation form: any `@{name}` / `@{name=default}` occurrences.
    if s.contains("@{") {
        return interpolate(s, props).map(J::String);
    }
    Some(J::String(s.to_string()))
}

/// A `@name=default` fallback, typed by shape: `true`/`false` → bool (so a
/// default can feed a `flag`-read prop like `closable`), numeric-looking →
/// number, anything else (including empty) → string.
fn parse_default(d: &str) -> serde_json::Value {
    use serde_json::Value as J;
    match d {
        "true" => J::Bool(true),
        "false" => J::Bool(false),
        _ => match d.parse::<f64>().ok().and_then(serde_json::Number::from_f64) {
            Some(n) => J::Number(n),
            None => J::String(d.to_string()),
        },
    }
}

/// Replace every `@{name}` / `@{name=default}` occurrence in `s` with the prop's
/// text rendering (`52`, not `52.0`, for whole numbers — `f64`'s `Display`);
/// `None` (whole-value removal) when any referenced prop is absent with no default.
fn interpolate(s: &str, props: &HashMap<String, Value>) -> Option<String> {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(start) = rest.find("@{") {
        out.push_str(&rest[..start]);
        let body = &rest[start + 2..];
        let Some(end) = body.find('}') else {
            // Unterminated `@{` — keep the tail literally (best-effort).
            out.push_str(&rest[start..]);
            return Some(out);
        };
        let token = &body[..end];
        let (name, default) = match token.split_once('=') {
            Some((n, d)) => (n, Some(d)),
            None => (token, None),
        };
        match props.get(name) {
            Some(Value::Text(t)) => out.push_str(t),
            Some(Value::Number(n)) => out.push_str(&n.to_string()),
            Some(Value::Bool(b)) => out.push_str(if *b { "true" } else { "false" }),
            None => match default {
                Some(d) => out.push_str(d),
                None => return None,
            },
        }
        rest = &body[end + 1..];
    }
    out.push_str(rest);
    Some(out)
}

/// Splice the instance's slot content into a parsed proto tree: a `slot` node
/// (`component: "slot"`, prop `name`) is replaced IN PLACE by the named slot's
/// nodes; an empty/absent slot falls back to the slot node's own children; both
/// empty → nothing. A node with prop `when_filled: true` is dropped entirely when
/// no slot beneath it produced instance content (a `window` footer's
/// omit-when-empty). Returns the node's replacement sequence plus whether any
/// slot beneath produced instance (not fallback) content.
fn splice_slots(mut node: UiNode, slots: &mut Slots) -> (Vec<UiNode>, bool) {
    if node.component == "slot" {
        let name = crate::config::text(&node.props, "name").unwrap_or_default().to_string();
        let taken = slots.remove(&name).unwrap_or_default();
        if !taken.is_empty() {
            return (taken, true);
        }
        // Fallback children — themselves spliced (they may nest further slots).
        let mut out = Vec::new();
        let mut produced = false;
        for c in node.children {
            let (ns, p) = splice_slots(c, slots);
            produced |= p;
            out.extend(ns);
        }
        return (out, produced);
    }

    let mut produced = false;
    let mut kids = Vec::new();
    for c in std::mem::take(&mut node.children) {
        let (ns, p) = splice_slots(c, slots);
        produced |= p;
        kids.extend(ns);
    }
    node.children = kids;
    // A proto may nest a template reference whose OWN slot groups hold `slot`
    // nodes (`window` fills `frame.center` with a section containing slots) —
    // splice inside those groups too, before the nested template expands.
    for group in node.slots.values_mut() {
        let mut spliced = Vec::new();
        for c in std::mem::take(group) {
            let (ns, p) = splice_slots(c, slots);
            produced |= p;
            spliced.extend(ns);
        }
        *group = spliced;
    }

    if crate::config::flag(&node.props, "when_filled") {
        node.props.remove("when_filled");
        if !produced {
            return (Vec::new(), false);
        }
    }
    (vec![node], produced)
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

// Props-map readers — thin wrappers over the shared `config` surface, so the builders and
// the walker read a scalar the SAME way (one implementation, in `config`).
fn p_num(p: &HashMap<String, Value>, key: &str) -> Option<f64> {
    crate::config::num(p, key)
}

fn p_text<'a>(p: &'a HashMap<String, Value>, key: &str) -> Option<&'a str> {
    crate::config::text(p, key)
}

/// Read a boolean prop (absent / non-bool / literal `false` → `false`). Shared by
/// every builder that reads a scene flag (`has_min`, `disabled`, `divider`, …);
/// the string-and-number sibling of [`p_text`] / [`p_num`].
fn p_bool(p: &HashMap<String, Value>, key: &str) -> bool {
    crate::config::flag(p, key)
}

/// Pull a named slot's nodes out (leaving it empty), or an empty vec if absent.
fn take_slot(slots: &mut Slots, key: &str) -> Vec<UiNode> {
    slots.remove(key).unwrap_or_default()
}

// ── component builders (composed BY the templates below; one definition each) ──

/// **WindowControls** — the Component that occupies a closable frame's top-RIGHT corner:
/// a single ✕ button that fires `close_action`. ONE definition, composed by every surface
/// that needs modal window controls (the canonical "one Top Right Corner Component, used in
/// several Templates"). The [`frame`] injects it into the `ne` cell and suppresses the tr
/// rune so the ✕ owns the corner; `action` / `style` / `label` are the caller's config.
fn window_controls(action: &str, style: &str, label: &str) -> UiNode {
    let mut x = with_style(elem("button"), Some(style));
    x.action = Some(action.to_string());
    x.props.insert("label".to_string(), Value::Text(label.to_string()));
    x
}

// ── templates ────────────────────────────────────────────────────────────────

/// **Frame** — the universal border-grid CONTAINER: the chrome shell every window /
/// panel / dialog derives from, built OVER the `grid` layout kind as a 3×3 border
/// grid. Where `window` HAND-composes a title-bar row + content well + rune overlay
/// and leans on patch-props (`title_pad` to inset the title past the corner runes,
/// `w_frac` for a responsive size), `frame` makes that geometry STRUCTURAL: the
/// eight edge/corner regions are grid cells whose track widths ARE the decoration
/// clearance, so the centre content is inset past the corner runes BY CONSTRUCTION —
/// no `title_pad`. (Phase 2 of the Prism UI frame system; `window` / `workbench`
/// migrate onto it in Phase 3 — this builder is ADDITIVE and touches neither.)
///
/// The nine optional named region SLOTS map 1:1 onto the 3×3 grid — nine distinct,
/// NON-OVERLAPPING zones. Each region, including the `n` (title) and `s` (footer) bars, is
/// confined to its OWN single cell; the bars sit in the top-/bottom-CENTRE cell, flanked by
/// the corner cells that hold the runes, so the title never overlaps a corner:
///
/// ```text
///   nw = (0,0)   n = title  (1,0)   ne = (2,0)
///   w  = (0,1)   center     (1,1)    e = (2,1)
///   sw = (0,2)   s = footer (1,2)   se = (2,2)
/// ```
///
/// The column axis is `[w_size | 1fr | e_size]` and the row axis `[n_size | 1fr |
/// s_size]`. The four CORNER cells (`nw`/`ne`/`sw`/`se`) are the edge-track intersections
/// that hold the corner-rune clearance; the four EDGE cells (`n`/`s`/`w`/`e`) are the title
/// / footer / side borders; the `center` holds content, inset past the runes on every side
/// by the edge tracks BY CONSTRUCTION (the point of the frame). Every region is optional;
/// because all four edge tracks are `Fixed(px)` an absent region emits no grid child yet its
/// cell still reserves the space (a Fixed track is content-independent — sidestepping the
/// grid's Auto-track intrinsic sizing, so an empty edge never collapses the clearance).
///
/// The frame ROOT is a styled **`stack`** (like `window`) that OVERLAYS `[grid,
/// runes]`: the stack carries `settings.window` and draws the frame bg through
/// `draw_panel_bg`; the border grid is intentionally unstyled/structural (the bg
/// shows through every region); the `rune_corners` inlay fills the frame rect and is
/// the LAST child, so its glyphs paint OVER the region grid at the four corners.
///
/// Slots (all optional): `center`, `n`, `s`, `w`, `e`, `nw`, `ne`, `sw`, `se`. Props
/// (all optional): `style` (dotted-path PREFIX, default `"settings"` →
/// `settings.window` / `.runes`); `w` / `h` (fixed frame size in px) OR `w_frac` /
/// `h_frac` (a responsive fraction of the parent/screen — a fixed axis wins over its
/// `*_frac`); `edge` (default size, px, for ALL four edge tracks — default `30.0`,
/// which is `settings.runes` `inset(14)` + `size(16)`, the corner-rune box extent, so
/// ANY frame clears the runes intrinsically); and the per-edge overrides `n_size`
/// (title-bar height), `s_size` (footer height), `w_size` / `e_size` (side borders),
/// each defaulting to `edge`. The `>= edge` floor is a clearance INVARIANT, not a
/// clamp: a scene may pass a thinner border deliberately (a frame with no runes), but
/// a value below ~30 reintroduces the exact top-corner rune collision `window`'s
/// `title_pad` used to hide. Title / button / footer / body-pad props live in the
/// region-subtree content, NOT here — `frame` is pure chrome geometry. `col_gap` /
/// `row_gap` / `pad` stay at the node default `0.0`: the tracks must be flush against
/// the frame edge and each other for the corner cells to sit exactly at the corners.
/// An undersized frame (`w_size + e_size > width`, or `n_size + s_size > height`)
/// yields a negative-extent centre cell (free is unclamped, as in flow), so a
/// responsive `w_frac` / `h_frac` must stay large enough to exceed the summed edges.
fn frame(_ctx: &BuildCtx, p: &HashMap<String, Value>, slots: &mut Slots) -> UiNode {
    // Dotted-path PREFIX → the reused style blocks (settings.* by default).
    let prefix = p_text(p, "style").unwrap_or("settings").to_string();
    let sty = |leaf: &str| format!("{prefix}.{leaf}");

    // ── edge track sizes → the 3×3 border grid's cols / rows ──
    // `edge` is the STRUCTURAL clearance constant: settings.runes inset(14) +
    // size(16) ≈ 30, the corner-rune box extent. It is a Rust literal, NOT read from
    // the token block — colours/geometry never cross into a builder and the theme is
    // not available here (exactly as the scenes pass a literal title_pad = 30). Any
    // edge >= this value clears the corner runes on that side by construction.
    let edge = p_num(p, "edge").unwrap_or(30.0) as f32;
    let n = p_num(p, "n_size").map(|v| v as f32).unwrap_or(edge);
    let s = p_num(p, "s_size").map(|v| v as f32).unwrap_or(edge);
    let w = p_num(p, "w_size").map(|v| v as f32).unwrap_or(edge);
    let e = p_num(p, "e_size").map(|v| v as f32).unwrap_or(edge);

    // ── closable: the top-RIGHT corner becomes an ✕ close control instead of a rune ──
    // A dismissible frame swaps its `ne` corner rune for an ✕ button (fires `close_action`);
    // the rune overlay then suppresses its `tr` glyph so the two never stack. The button is
    // injected into the `ne` slot, so the region loop places it in the ne cell (e × n_size).
    // This is the ONE corner-control capability this pass (Aaron: "replace the top-right rune
    // with X when the panel can be closed"); move / resize stay for the Window-system phase.
    let closable = p_bool(p, "closable");
    if closable {
        let close_style = p_text(p, "close_style").map(|s| s.to_string()).unwrap_or_else(|| sty("titlebar.close"));
        slots.entry("ne".to_string()).or_default().push(window_controls(
            p_text(p, "close_action").unwrap_or("close"),
            &close_style,
            p_text(p, "close_label").unwrap_or("×"),
        ));
    }

    // ── the border grid: structural (unstyled), fills the frame, flush tracks ──
    // `f32` Display renders 30.0 as "30" and 52.5 as "52.5" — both tokens
    // `parse_track` reads as `Fixed`; the "1fr" centre parses as `Fr(1.0)`.
    let mut grid = with_text(elem("grid"), "cols", &format!("{w} 1fr {e}"));
    grid = with_text(grid, "rows", &format!("{n} 1fr {s}"));
    grid.anchor = Some(UiAnchor::TopLeft);
    grid = with_num(grid, "width_frac", 1.0);
    grid = with_num(grid, "height_frac", 1.0);

    // ── stamp each present region into its cell, in a FIXED emission order ──
    // (nw, n, ne, w, center, e, sw, s, se) for deterministic child / draw ordering.
    // Every region stamps BOTH `col` AND `row`: a grid child is "explicit" if it
    // carries either, and the missing axis would silently default to 0 — so both
    // always cross. A single-node region fills its cell exactly; a multi-node region
    // stamps each node, which grid stacks overlapping in the same cell (the
    // documented stack case) — no wrapper kind is introduced.
    let regions: [(&str, f64, f64); 9] = [
        ("nw", 0.0, 0.0), ("n", 1.0, 0.0), ("ne", 2.0, 0.0),
        ("w", 0.0, 1.0), ("center", 1.0, 1.0), ("e", 2.0, 1.0),
        ("sw", 0.0, 2.0), ("s", 1.0, 2.0), ("se", 2.0, 2.0),
    ];
    for (name, col, row) in regions {
        // ONE region → ONE cell. Every named region — INCLUDING the `n` title bar and the
        // `s` footer — is confined to its own single grid cell, giving nine distinct,
        // non-overlapping zones (a true 3×3 border grid). The `n` bar sits in the top-CENTRE
        // cell (1,0), flanked by the `nw` / `ne` corner cells that hold the runes, so the
        // title can never overlap a corner rune (the collision a full-bleed `n`/`s` span
        // reintroduced). A multi-node region stamps every node into the SAME cell, which grid
        // stacks overlapping (the documented stack case) — no wrapper kind is introduced.
        for node in take_slot(slots, name) {
            grid.children.push(with_num(with_num(node, "col", col), "row", row));
        }
    }

    // ── rune-inlay overlay: fills the frame; glyphs at the four corners ──
    let mut runes = with_style(elem("rune_corners"), Some(sty("runes").as_str()));
    runes.anchor = Some(UiAnchor::TopLeft);
    runes = with_num(runes, "width_frac", 1.0);
    runes = with_num(runes, "height_frac", 1.0);
    // A closable frame's ✕ owns the top-right corner — blank the `tr` glyph so it doesn't
    // paint beneath the button.
    if closable {
        runes = with_text(runes, "tr", "");
    }

    // ── frame: a styled STACK overlaying the border grid + the rune inlays ──
    // The stack itself carries `settings.window` and draws through `draw_panel_bg`
    // (a styled stack == a styled panel in look) — that IS the frame bg, identical to
    // `window`. Size is a fixed `w` / `h` in px OR a RESPONSIVE `w_frac` / `h_frac`
    // fraction of the parent; a fixed axis wins, `*_frac` fills in only the axis left
    // unset. Self-pins Center + its own size (like `window`), so a scene cannot
    // re-anchor the modal through `overlay_placement`.
    let mut frame = with_style(elem("stack"), Some(sty("window").as_str()));
    frame.anchor = Some(UiAnchor::Center);
    frame.width = p_num(p, "w").map(|v| v as f32);
    frame.height = p_num(p, "h").map(|v| v as f32);
    if frame.width.is_none() {
        if let Some(wf) = p_num(p, "w_frac") {
            frame = with_num(frame, "width_frac", wf);
        }
    }
    if frame.height.is_none() {
        if let Some(hf) = p_num(p, "h_frac") {
            frame = with_num(frame, "height_frac", hf);
        }
    }
    frame.children = vec![grid, runes];
    frame
}

/// **Card** — a Prism carved-stone slab: a styled `cell` wrapping a `cell`
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
    let mut body = elem("cell");
    body.gap = p_num(p, "gap").unwrap_or(10.0) as f32;
    let mut body_kids: Vec<UiNode> = Vec::new();
    if !header_kids.is_empty() {
        let mut header = elem("cell");
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
    let mut panel = with_style(elem("cell"), Some(p_text(p, "style").unwrap_or("menu.panel")));
    panel.pad = p_num(p, "pad").unwrap_or(16.0) as f32;
    panel.children = vec![body];
    panel
}

/// **OptionGrid** — a flowing FIELD of clickable option cards: the reusable "pick a task from a
/// field of options" control. The import WORKFLOW selector is one instance; the scene selector is
/// the same idea in a second layout form. A scene LOOPS its own list and builds one option node per
/// entry (a `card` tile under a click `button`, or any node it likes), passing them as the `cards`
/// slot; this template arranges them into rows of `cols` inside a recessed WELL, under an optional
/// caps `heading` and italic `subtitle`, with an optional `hint` pinned to the bottom. Arbitrary
/// count → more rows. The field GROWS to fill its container (e.g. a `window` well), so the selector
/// scales with the modal instead of the modal shrinking to the cards.
///
/// Slot: `cards` (the option nodes — the scene owns each tile's look and size). Props (all
/// optional): `cols` (per row, default 4); `heading` + `heading_size` + `heading_color`;
/// `subtitle` + `subtitle_size` + `subtitle_color` (wraps); `hint` + `hint_size` + `hint_color`;
/// `gap` (between heading / well / hint), `grid_gap` (between cards), `well_pad`, and the dotted
/// `well_style` path. Only dotted style/colour PATHS cross here — never a colour.
fn option_grid(_ctx: &BuildCtx, p: &HashMap<String, Value>, slots: &mut Slots) -> UiNode {
    let cols = (p_num(p, "cols").unwrap_or(4.0) as usize).max(1);
    let grid_gap = p_num(p, "grid_gap").unwrap_or(20.0) as f32;

    // One text line: `size` is the row height it reserves in the flow (enough for `lines` lines),
    // `text_size` the glyph size. A wrapped line breaks to the column width over its reserved height.
    let line = |text: &str, glyph: f64, lines: f64, color: &str, font: &str, italic: bool, wrap: bool, tracking: f64| -> UiNode {
        let mut n = elem("text");
        n.size = Some((glyph * 1.3 * lines) as f32);
        n = with_num(n, "text_size", glyph);
        n.props.insert("text".to_string(), Value::Text(text.to_string()));
        n.props.insert("color".to_string(), Value::Text(color.to_string()));
        n.props.insert("font".to_string(), Value::Text(font.to_string()));
        if italic {
            n.props.insert("italic".to_string(), Value::Bool(true));
        }
        if wrap {
            n.props.insert("wrap".to_string(), Value::Bool(true));
        }
        if tracking > 0.0 {
            n = with_num(n, "tracking", tracking);
        }
        n
    };

    // Caps tracking: a MODEST letter-spacing for section headers (the `label` face already tracks
    // 0.16 by default). A scene can override per line; the defaults stay tight — never the 2-em
    // spread that read as "extra wide".
    let head_track = p_num(p, "heading_tracking").unwrap_or(0.22);
    let hint_track = p_num(p, "hint_tracking").unwrap_or(0.16);
    let mut col: Vec<UiNode> = Vec::new();
    if let Some(h) = p_text(p, "heading") {
        col.push(line(h, p_num(p, "heading_size").unwrap_or(12.0), 1.0, p_text(p, "heading_color").unwrap_or("modal.subtitle.color"), "label", false, false, head_track));
    }
    if let Some(s) = p_text(p, "subtitle") {
        col.push(line(s, p_num(p, "subtitle_size").unwrap_or(13.0), 2.0, p_text(p, "subtitle_color").unwrap_or("modal.subtitle.color"), "body", true, true, 0.0));
    }

    // Chunk the author-built option cards into rows of `cols` (the walker has no wrap engine, so the
    // grid is authored as rows). Consumes the slot so nodes move, never clone.
    let mut rows: Vec<UiNode> = Vec::new();
    let mut it = take_slot(slots, "cards").into_iter().peekable();
    while it.peek().is_some() {
        let mut kids: Vec<UiNode> = Vec::new();
        for _ in 0..cols {
            match it.next() {
                Some(c) => kids.push(c),
                None => break,
            }
        }
        let mut row = elem("row");
        row.gap = grid_gap;
        row.children = kids;
        rows.push(row);
    }
    let mut grid = elem("cell");
    grid.gap = grid_gap;
    grid.children = rows;

    // The recessed well the cards sit in — grows to fill the field so the hint pins to the bottom.
    let mut well = with_style(elem("cell"), p_text(p, "well_style"));
    well.grow = Some(1.0);
    well.pad = p_num(p, "well_pad").unwrap_or(24.0) as f32;
    well.children = vec![grid];
    col.push(well);

    if let Some(hint) = p_text(p, "hint") {
        col.push(line(hint, p_num(p, "hint_size").unwrap_or(11.0), 1.0, p_text(p, "hint_color").unwrap_or("modal.subtitle.color"), "label", false, false, hint_track));
    }

    // Root: a column that grows to fill its container (the window well), so the field tracks the
    // modal's size — an arbitrary number of cards flows into more rows rather than resizing the modal.
    let mut root = elem("cell");
    root.grow = Some(1.0);
    root.gap = p_num(p, "gap").unwrap_or(14.0) as f32;
    root.children = col;
    root
}

#[cfg(test)]
mod tests {
    use super::*;

    fn leaf(component: &str) -> UiNode {
        elem(component)
    }

    /// Props map from `(key, Value)` pairs — the test-side twin of a scene's node table.
    fn props_of(pairs: &[(&str, Value)]) -> HashMap<String, Value> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.clone())).collect()
    }

    /// A one-entry Data registry over a leaked (test-'static) proto value.
    fn data_reg(name: &str, proto: serde_json::Value) -> TemplateRegistry {
        let mut reg: TemplateRegistry = HashMap::new();
        reg.insert(name.to_string(), TemplateDef::Data(Box::leak(Box::new(proto))));
        reg
    }

    // ── the substitution engine (`substitute` / `when` / interpolation) ──────────

    /// Exact-form `@name` substitution is TYPED: a bool / number / text prop lands as
    /// its native JSON value, and a `@name=default` fills absence — `true`/`false` as
    /// bool, numeric-looking as number, else string, `=` alone as empty string.
    #[test]
    fn substitute_exact_form_is_typed_and_defaults_fill_absence() {
        let props = props_of(&[
            ("flag", Value::Bool(false)),
            ("n", Value::Number(52.0)),
            ("t", Value::Text("Prism".into())),
        ]);
        let proto = serde_json::json!({
            "flag": "@flag", "n": "@n", "t": "@t",
            "dn": "@missing=12", "dt": "@missing=label", "de": "@missing=",
            "db": "@missing=true"
        });
        let out = substitute(&proto, &props).expect("object survives");
        assert_eq!(out["flag"], serde_json::json!(false), "bool rides native");
        assert_eq!(out["n"], serde_json::json!(52.0), "number rides native");
        assert_eq!(out["t"], serde_json::json!("Prism"), "text rides native");
        assert_eq!(out["dn"], serde_json::json!(12.0), "numeric-looking default → number");
        assert_eq!(out["dt"], serde_json::json!("label"), "non-numeric default → string");
        assert_eq!(out["de"], serde_json::json!(""), "`=` alone → empty string");
        assert_eq!(out["db"], serde_json::json!(true), "`true` default → bool (a `flag` prop)");
    }

    /// An absent prop with NO default removes its holder: the object key vanishes and
    /// the array element is dropped (a builder's `if let Some(..)` arms, as data).
    #[test]
    fn substitute_absent_prop_removes_key_and_array_element() {
        let props = props_of(&[("kept", Value::Number(1.0))]);
        let proto = serde_json::json!({
            "kept": "@kept",
            "gone": "@missing",
            "list": ["@kept", "@missing", "tail"]
        });
        let out = substitute(&proto, &props).expect("object survives");
        assert!(out.get("gone").is_none(), "absent + no default → the key is removed");
        assert_eq!(out["list"], serde_json::json!([1.0, "tail"]), "the element is dropped");
    }

    /// `@{name}` interpolation replaces every occurrence with the prop's text
    /// rendering (whole numbers render bare: `52`, not `52.0`); a default fills
    /// absence; an absent name with no default removes the WHOLE value.
    #[test]
    fn substitute_interpolates_strings_and_removes_on_absent() {
        let props = props_of(&[
            ("variant", Value::Text("danger".into())),
            ("n", Value::Number(52.0)),
        ]);
        let proto = serde_json::json!({
            "style": "modal.buttons.variants.@{variant=primary}",
            "fallback": "modal.buttons.variants.@{missing=primary}",
            "multi": "@{variant}-@{n}",
            "gone": "x-@{missing}"
        });
        let out = substitute(&proto, &props).expect("object survives");
        assert_eq!(out["style"], serde_json::json!("modal.buttons.variants.danger"));
        assert_eq!(out["fallback"], serde_json::json!("modal.buttons.variants.primary"));
        assert_eq!(out["multi"], serde_json::json!("danger-52"), "numbers render bare");
        assert!(out.get("gone").is_none(), "absent interp with no default removes the value");
    }

    /// `when` gates a node on prop truthiness (present ∧ not `false` ∧ not empty
    /// text); `!@name` negates; a passing gate strips the `when` key.
    #[test]
    fn substitute_when_gates_drop_nodes_and_strip_the_key() {
        let props = props_of(&[
            ("subtitle", Value::Text("THE SEVEN SHARDS".into())),
            ("divider", Value::Bool(false)),
            ("empty", Value::Text(String::new())),
        ]);
        let proto = serde_json::json!({ "children": [
            { "component": "text", "when": "@subtitle" },
            { "component": "cell", "when": "@divider" },
            { "component": "cell", "when": "@empty" },
            { "component": "cell", "when": "@missing" },
            { "component": "row", "when": "!@divider" },
            { "component": "grid", "when": "!@subtitle" }
        ]});
        let out = substitute(&proto, &props).expect("object survives");
        let kids = out["children"].as_array().expect("children array");
        let kinds: Vec<&str> =
            kids.iter().map(|k| k["component"].as_str().unwrap_or("?")).collect();
        assert_eq!(kinds, vec!["text", "row"], "false-bool / empty-text / absent all drop; negation inverts");
        assert!(kids[0].get("when").is_none(), "a passing gate strips its `when` key");
    }

    /// `$token` / `$$` strings are stringtable refs resolved at DRAW — the
    /// substitution engine must pass them through untouched, never double-process.
    #[test]
    fn substitute_passes_stringtable_refs_through_untouched() {
        let props = props_of(&[("t", Value::Text("x".into()))]);
        let proto = serde_json::json!({ "a": "$menu_quit", "b": "$$5.00", "c": "$with @{t} inside" });
        let out = substitute(&proto, &props).expect("object survives");
        assert_eq!(out["a"], serde_json::json!("$menu_quit"));
        assert_eq!(out["b"], serde_json::json!("$$5.00"));
        assert_eq!(out["c"], serde_json::json!("$with @{t} inside"), "a `$` string is NEVER interpolated");
    }

    // ── slot splice + `when_filled` + the depth guard ────────────────────────────

    /// A `slot` node is replaced by the instance's named content; an empty slot
    /// falls back to the slot node's own children; both empty → nothing.
    #[test]
    fn data_slot_splices_content_and_falls_back() {
        let proto = serde_json::json!({ "component": "cell", "children": [
            { "component": "slot", "name": "items", "children": [ { "component": "text" } ] },
            { "component": "slot", "name": "extra" }
        ]});
        let reg = data_reg("holder", proto);

        // Filled: the two buttons replace the slot node (fallback discarded).
        let filled = template_node("holder", vec![("items", vec![leaf("button"), leaf("button")])]);
        let out = expand(filled, &reg);
        assert_eq!(out.component, "cell");
        assert_eq!(out.children.len(), 2);
        assert!(out.children.iter().all(|c| c.component == "button"));

        // Empty: the `items` fallback text stands in; `extra` (no fallback) yields nothing.
        let out = expand(template_node("holder", vec![]), &reg);
        assert_eq!(out.children.len(), 1);
        assert_eq!(out.children[0].component, "text");
    }

    /// `when_filled: true` drops its node when no slot beneath produced INSTANCE
    /// content (fallback does not count), and the marker prop never leaks through.
    #[test]
    fn data_when_filled_drops_an_unfilled_wrapper() {
        let proto = serde_json::json!({ "component": "cell", "children": [
            { "component": "row", "when_filled": true, "children": [
                { "component": "slot", "name": "footer" }
            ]}
        ]});
        let reg = data_reg("w", proto);

        let out = expand(template_node("w", vec![("footer", vec![leaf("button")])]), &reg);
        assert_eq!(out.children.len(), 1, "a filled slot keeps the wrapper");
        assert_eq!(out.children[0].component, "row");
        assert!(!out.children[0].props.contains_key("when_filled"), "the marker is stripped");

        let out = expand(template_node("w", vec![]), &reg);
        assert!(out.children.is_empty(), "an unfilled `when_filled` wrapper is dropped");
    }

    /// A self-referential data proto trips [`MAX_TEMPLATE_DEPTH`] and falls back to
    /// the same empty screen as an unknown template — never a hang or a panic.
    #[test]
    fn data_depth_guard_stops_a_self_referential_proto() {
        let proto = serde_json::json!({ "component": "cell", "children": [ { "template": "loop" } ] });
        let reg = data_reg("loop", proto);
        let mut node = template_node("loop", vec![]);
        node.id = "keepme".to_string();
        let out = expand(node, &reg);
        // The chain expands MAX_TEMPLATE_DEPTH times, then the innermost falls back.
        fn depth_of(n: &UiNode) -> usize {
            n.children.first().map(|c| 1 + depth_of(c)).unwrap_or(0)
        }
        assert_eq!(depth_of(&out), MAX_TEMPLATE_DEPTH, "8 cells then the fallback leaf");
        fn innermost(n: &UiNode) -> &UiNode {
            n.children.first().map(innermost).unwrap_or(n)
        }
        assert_eq!(innermost(&out).component, "screen", "the guard stands in the empty page");
        assert_eq!(out.component, "cell");
        assert_eq!(out.id, "keepme", "the instance id still lands via overlay_placement");
    }

    /// The structural pseudo-props: a proto reads the instance's `anchor` (consumed
    /// structurally by the parsers, so never a real prop) and `id` (the data twin of
    /// `BuildCtx::id_prefix`) through the same `@` forms as any prop.
    #[test]
    fn data_pseudo_props_expose_instance_anchor_and_id() {
        let proto = serde_json::json!({ "component": "cell", "children": [
            { "component": "rtt", "id": "@{id}_left", "anchor": "@anchor=center" }
        ]});
        let reg = data_reg("stage", proto);
        let mut node = template_node("stage", vec![]);
        node.id = "cmp".to_string();
        node.anchor = Some(UiAnchor::Left);
        let out = expand(node, &reg);
        let inner = &out.children[0];
        assert_eq!(inner.id, "cmp_left", "`@{{id}}` interpolates the instance id");
        assert_eq!(inner.anchor, Some(UiAnchor::Left), "`@anchor` reads the structural anchor");

        // Without an instance anchor the default holds.
        let mut plain = template_node("stage", vec![]);
        plain.id = "cmp".to_string();
        let out = expand(plain, &reg);
        assert_eq!(out.children[0].anchor, Some(UiAnchor::Center), "absent anchor → the default");
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
        let tree = container("cell", vec![leaf("text"), leaf("button")]);
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
                ("viewport", vec![leaf("rtt")]),
                ("rail", vec![leaf("cell")]),
                ("footer", vec![leaf("button")]),
            ],
        );
        let out = expand(node, &reg);
        // Root is the full-screen `frame` stack overlaying [grid, runes]; the grid's `center`
        // cell holds the SECTION (a vertical `cell`) with 4 rows: header · tabs · body · footer.
        assert_eq!(out.component, "stack");
        assert_eq!(out.children.len(), 2);
        let grid = &out.children[0];
        assert_eq!(grid.component, "grid");
        assert_eq!(grid.children.len(), 1, "only the center region (the section) is placed");
        let section = &grid.children[0];
        assert_eq!(section.component, "cell");
        assert_eq!(p_num(&section.props, "col"), Some(1.0));
        assert_eq!(p_num(&section.props, "row"), Some(1.0));
        assert_eq!(section.children.len(), 4);
        let (header, tabs, body, footer) =
            (&section.children[0], &section.children[1], &section.children[2], &section.children[3]);
        // header row carries its one slot child.
        assert_eq!(header.component, "row");
        assert_eq!(header.children.len(), 1);
        assert_eq!(header.children[0].component, "text");
        // tabs row carries both buttons.
        assert_eq!(tabs.children.len(), 2);
        // body row = viewport ++ rail, in order; grows to fill the middle.
        assert_eq!(body.children.len(), 2);
        assert_eq!(body.children[0].component, "rtt");
        assert_eq!(body.children[1].component, "cell");
        assert_eq!(body.grow, Some(1.0));
        // footer = panel > row > [button].
        assert_eq!(footer.component, "cell");
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
        // The outer workbench is a frame stack; the inner one (nested in the viewport slot)
        // expanded to its OWN frame stack — grid → section cell → body row → inner.
        assert_eq!(out.component, "stack");
        let inner = &out.children[0].children[0].children[2].children[0];
        assert_eq!(inner.component, "stack", "the nested workbench expanded to its own frame");
    }

    #[test]
    fn unknown_template_falls_back_to_an_empty_page() {
        let reg = builtin_templates();
        let mut node = template_node("does_not_exist", vec![]);
        node.id = "keepme".to_string();
        let out = expand(node, &reg);
        assert_eq!(out.component, "screen");
        assert_eq!(out.id, "keepme");
        assert!(out.children.is_empty());
    }

    #[test]
    fn overlay_places_the_instance_when_the_builder_is_neutral() {
        // A builder that returns a bare panel; the scene pins it center.
        fn bare(_c: &BuildCtx, _p: &HashMap<String, Value>, _s: &mut Slots) -> UiNode {
            elem("cell")
        }
        let mut reg: TemplateRegistry = HashMap::new();
        reg.insert("bare".to_string(), TemplateDef::Builder(bare));
        let mut node = elem("");
        node.template = Some("bare".to_string());
        node.anchor = Some(UiAnchor::Center);
        node.width = Some(400.0);
        let out = expand(node, &reg);
        assert_eq!(out.component, "cell");
        assert_eq!(out.anchor, Some(UiAnchor::Center));
        assert_eq!(out.width, Some(400.0));
    }

    #[test]
    fn window_frames_header_content_footer_section_over_the_frame() {
        let reg = builtin_templates();
        let mut node = template_node(
            "window",
            vec![
                ("content", vec![leaf("rtt")]),
                ("footer", vec![leaf("button")]),
            ],
        );
        node.props.insert("title".to_string(), Value::Text("Grimoire".to_string()));
        node.props.insert("w".to_string(), Value::Number(640.0));
        node.props.insert("h".to_string(), Value::Number(480.0));
        let out = expand(node, &reg);

        // Root = the frame STACK, sized + centred, overlaying [border grid, runes].
        assert_eq!(out.component, "stack");
        assert_eq!(p_text(&out.props, "style"), Some("settings.window"));
        assert_eq!(out.anchor, Some(UiAnchor::Center));
        assert_eq!(out.width, Some(640.0));
        assert_eq!(out.height, Some(480.0));
        assert_eq!(out.children.len(), 2);

        let (grid, runes) = (&out.children[0], &out.children[1]);
        assert_eq!(runes.component, "rune_corners");
        // Closable by default → the tr (top-right) rune is blanked; the ✕ owns that corner.
        assert_eq!(p_text(&runes.props, "tr"), Some(""));

        // The grid holds the ne ✕ close button (col 2, row 0) + the SECTION (center, col 1, row 1).
        assert_eq!(grid.component, "grid");
        assert_eq!(grid.children.len(), 2);
        let close = grid.children.iter().find(|c| c.component == "button").expect("ne close button");
        assert_eq!(p_num(&close.props, "col"), Some(2.0));
        assert_eq!(p_num(&close.props, "row"), Some(0.0));
        assert_eq!(close.action.as_deref(), Some("close")); // default close_action
        assert_eq!(p_text(&close.props, "label"), Some("×"));

        let section = grid.children.iter().find(|c| c.component == "cell").expect("center section");
        assert_eq!(p_num(&section.props, "col"), Some(1.0));
        assert_eq!(p_num(&section.props, "row"), Some(1.0));
        assert_eq!(section.children.len(), 3, "header · content · footer");
        let (header, content, footer) = (&section.children[0], &section.children[1], &section.children[2]);

        // HEADER cell — a titlebar-styled bar holding the title text.
        assert_eq!(header.component, "cell");
        assert_eq!(p_text(&header.props, "style"), Some("settings.titlebar"));
        assert_eq!(p_text(&header.children[0].props, "text"), Some("Grimoire"));

        // CONTENT cell — grows to fill the middle, wraps the `content` slot verbatim.
        assert_eq!(content.component, "cell");
        assert_eq!(content.grow, Some(1.0));
        assert_eq!(content.children[0].component, "rtt");

        // FOOTER cell — wraps a row of the footer slot button.
        assert_eq!(footer.component, "cell");
        assert_eq!(footer.children[0].component, "row");
        assert_eq!(footer.children[0].children[0].component, "button");
    }

    #[test]
    fn window_omits_the_footer_cell_when_the_slot_is_empty() {
        let reg = builtin_templates();
        // Only a content slot; no footer.
        let node = template_node("window", vec![("content", vec![leaf("text")])]);
        let out = expand(node, &reg);
        let grid = &out.children[0];
        assert_eq!(grid.component, "grid");
        // ne ✕ (closable default) + the center section.
        assert_eq!(grid.children.len(), 2);
        let section = grid.children.iter().find(|c| c.component == "cell").expect("center section");
        // header + content only — no footer cell when the footer slot is empty.
        assert_eq!(section.children.len(), 2);
        assert_eq!(section.children[0].component, "cell"); // header
        assert_eq!(section.children[1].component, "cell"); // content
        assert_eq!(section.children[1].children[0].component, "text"); // the content slot
    }

    /// A `frame` with `center` / `n` / corner slots expands to a styled `stack`
    /// overlaying a 3×3 border `grid` (`cols` / `rows` from the edge props) + a rune
    /// overlay, and each region is spliced into its cell carrying `col` / `row`.
    #[test]
    fn frame_emits_border_grid_with_named_region_cells() {
        let reg = builtin_templates();
        let mut node = template_node(
            "frame",
            vec![
                ("center", vec![leaf("rtt")]),
                ("n", vec![leaf("text")]),
                ("nw", vec![leaf("cell")]),
                ("se", vec![leaf("cell")]),
            ],
        );
        node.props.insert("w".into(), Value::Number(640.0));
        node.props.insert("h".into(), Value::Number(480.0));
        node.props.insert("n_size".into(), Value::Number(52.0));
        node.props.insert("s_size".into(), Value::Number(58.0));
        let out = expand(node, &reg);

        // Root: the styled frame STACK, sized + centred, overlaying [grid, runes].
        assert_eq!(out.component, "stack");
        assert_eq!(p_text(&out.props, "style"), Some("settings.window"));
        assert_eq!(out.anchor, Some(UiAnchor::Center));
        assert_eq!(out.width, Some(640.0));
        assert_eq!(out.height, Some(480.0));
        assert_eq!(out.children.len(), 2);

        // Border grid: unstyled, fills the frame, w/e default 30, n=52 / s=58.
        let (grid, runes) = (&out.children[0], &out.children[1]);
        assert_eq!(grid.component, "grid");
        assert_eq!(p_text(&grid.props, "cols"), Some("30 1fr 30"));
        assert_eq!(p_text(&grid.props, "rows"), Some("52 1fr 58"));
        assert_eq!(grid.anchor, Some(UiAnchor::TopLeft));
        assert_eq!(p_num(&grid.props, "width_frac"), Some(1.0));
        assert_eq!(p_num(&grid.props, "height_frac"), Some(1.0));

        // Rune overlay: last child, points at the reused runes block.
        assert_eq!(runes.component, "rune_corners");
        assert_eq!(p_text(&runes.props, "style"), Some("settings.runes"));

        // Emission order (nw, n, center, se here) is fixed; each region is at its cell.
        let kinds: Vec<&str> = grid.children.iter().map(|c| c.component.as_str()).collect();
        assert_eq!(kinds, vec!["cell", "text", "rtt", "cell"], "fixed emission order");
        let at = |kind: &str, col: f64, row: f64| {
            let c = grid.children.iter().find(|c| c.component == kind).unwrap();
            assert_eq!(p_num(&c.props, "col"), Some(col), "{kind} col");
            assert_eq!(p_num(&c.props, "row"), Some(row), "{kind} row");
        };
        at("cell", 0.0, 0.0); // nw is the FIRST panel in order
        at("text", 1.0, 0.0); // n — the title bar in its own top-CENTRE cell (1,0)
        assert_eq!(p_num(&grid.children[1].props, "col_span"), None, "the n bar is confined to one cell, not full-bleed");
        at("rtt", 1.0, 1.0); // center — the inset content cell
        // se is the SECOND panel — index 3 in emission order.
        let se = &grid.children[3];
        assert_eq!(p_num(&se.props, "col"), Some(2.0));
        assert_eq!(p_num(&se.props, "row"), Some(2.0));
    }

    /// With no edge props at all, every edge defaults to the rune-clearance constant
    /// (30), and a `w_frac` rides the responsive width_frac path (root stays unsized).
    #[test]
    fn frame_defaults_edges_to_rune_clearance_and_is_responsive() {
        let reg = builtin_templates();
        let mut node = template_node("frame", vec![("center", vec![leaf("rtt")])]);
        node.props.insert("w_frac".into(), Value::Number(0.82));
        let out = expand(node, &reg);

        let grid = &out.children[0];
        assert_eq!(p_text(&grid.props, "cols"), Some("30 1fr 30"));
        assert_eq!(p_text(&grid.props, "rows"), Some("30 1fr 30"));
        // Responsive: no fixed width, width_frac rides the anchored() path.
        assert_eq!(out.width, None);
        assert_eq!(p_num(&out.props, "width_frac"), Some(0.82));
        // Only the centre region is placed, at cell (1,1).
        assert_eq!(grid.children.len(), 1);
        assert_eq!(p_num(&grid.children[0].props, "col"), Some(1.0));
        assert_eq!(p_num(&grid.children[0].props, "row"), Some(1.0));
    }

    /// An absent region emits no grid child (a Fixed edge track still reserves its
    /// cell); only the supplied regions carry `col` / `row`.
    #[test]
    fn frame_omits_absent_regions() {
        let reg = builtin_templates();
        let node = template_node(
            "frame",
            vec![("center", vec![leaf("rtt")]), ("n", vec![leaf("text")])],
        );
        let out = expand(node, &reg);
        let grid = &out.children[0];
        // Emission order → n (the title bar in its own centre cell: col 1, row 0) then center (1,1).
        assert_eq!(grid.children.len(), 2);
        assert_eq!(grid.children[0].component, "text");
        assert_eq!(p_num(&grid.children[0].props, "col"), Some(1.0), "the n bar sits in the top-centre cell");
        assert_eq!(p_num(&grid.children[0].props, "row"), Some(0.0));
        assert_eq!(p_num(&grid.children[0].props, "col_span"), None, "the n bar is confined to one cell, not full-bleed");
        assert_eq!(grid.children[1].component, "rtt");
        assert_eq!(p_num(&grid.children[1].props, "col"), Some(1.0), "the centre is a single inset cell");
        assert_eq!(p_num(&grid.children[1].props, "row"), Some(1.0));
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
        assert_eq!(out.component, "cell");
        assert_eq!(
            out.props.get("style"),
            Some(&Value::Text("menu.panel".to_string()))
        );

        // panel > column(body) > [ header column, ...content ]
        assert_eq!(out.children.len(), 1);
        let body = &out.children[0];
        assert_eq!(body.component, "cell");
        assert_eq!(body.children.len(), 3);

        // Header groups the title (display face) over the subtitle (label caps).
        let header = &body.children[0];
        assert_eq!(header.component, "cell");
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
            .insert("content".to_string(), vec![leaf("cell")]);
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
    fn option_grid_chunks_an_arbitrary_card_count_into_rows_with_heading_well_and_hint() {
        let reg = builtin_templates();
        // FIVE option cards at 2 columns — the whole point is an ARBITRARY count: they must flow into
        // rows of 2, 2, 1, never resize the field to the cards.
        let mut node = template_node(
            "option_grid",
            vec![(
                "cards",
                vec![leaf("button"), leaf("button"), leaf("button"), leaf("button"), leaf("button")],
            )],
        );
        node.props.insert("cols".into(), Value::Number(2.0));
        node.props.insert("heading".into(), Value::Text("CHOOSE A WORKFLOW".into()));
        node.props.insert("subtitle".into(), Value::Text("pick one to begin".into()));
        node.props.insert("hint".into(), Value::Text("SELECT TO BEGIN".into()));
        node.props.insert("well_style".into(), Value::Text("assetpipeline.well".into()));

        let out = expand(node, &reg);

        // Root: a GROWING column (fills the window well) — heading · subtitle · well · hint.
        assert_eq!(out.component, "cell");
        assert_eq!(out.grow, Some(1.0), "the field grows to fill its container, not the reverse");
        assert_eq!(out.children.len(), 4);
        let (heading, subtitle, well, hint) =
            (&out.children[0], &out.children[1], &out.children[2], &out.children[3]);

        assert_eq!(heading.component, "text");
        assert_eq!(heading.props.get("text"), Some(&Value::Text("CHOOSE A WORKFLOW".into())));
        assert_eq!(subtitle.props.get("text"), Some(&Value::Text("pick one to begin".into())));
        assert_eq!(subtitle.props.get("wrap"), Some(&Value::Bool(true)), "the subtitle wraps");
        assert_eq!(hint.props.get("text"), Some(&Value::Text("SELECT TO BEGIN".into())));

        // Well: a growing styled panel holding the grid column of rows.
        assert_eq!(well.component, "cell");
        assert_eq!(well.grow, Some(1.0));
        assert_eq!(well.props.get("style"), Some(&Value::Text("assetpipeline.well".into())));
        let grid = &well.children[0];
        assert_eq!(grid.component, "cell");
        assert_eq!(grid.children.len(), 3, "5 cards / 2 cols = 3 rows");
        assert_eq!(grid.children[0].component, "row");
        assert_eq!(grid.children[0].children.len(), 2);
        assert_eq!(grid.children[2].children.len(), 1, "the last row holds the remainder");
    }

    #[test]
    fn popup_menu_composes_scrim_muse_and_layered_popup() {
        let reg = builtin_templates();
        // A left-hero menu popup: title + subtitle + divider + two launch buttons, a Muse
        // sprite behind, styles pointed at the shell's blocks. The hero placement rides
        // the instance's structural `anchor` (the `@anchor` pseudo-prop) + `offset_x`.
        let mut node = template_node(
            "popup_menu",
            vec![
                ("items", vec![leaf("button"), leaf("button")]),
                ("muse", vec![leaf("sprite")]),
            ],
        );
        node.anchor = Some(UiAnchor::Left);
        node.props.insert("offset_x".into(), Value::Number(150.0));
        node.props.insert("title".into(), Value::Text("Prism".into()));
        node.props.insert("title_size".into(), Value::Number(58.0));
        node.props.insert("subtitle".into(), Value::Text("THE SEVEN SHARDS".into()));
        node.props.insert("divider".into(), Value::Bool(true));
        node.props.insert("overlay_style".into(), Value::Text("screens.menu".into()));
        node.props.insert("panel_style".into(), Value::Text("modal.panel".into()));

        let out = expand(node, &reg);

        // Root is a full-bleed overlay screen: scrim, muse sprite, popup — in that order.
        assert_eq!(out.component, "screen");
        assert_eq!(out.anchor, Some(UiAnchor::TopLeft));
        assert_eq!(out.children.len(), 3);
        let (scrim, muse, popup) = (&out.children[0], &out.children[1], &out.children[2]);

        // Scrim: a full-bleed panel styled by `overlay_style`.
        assert_eq!(scrim.component, "cell");
        assert_eq!(scrim.props.get("style"), Some(&Value::Text("screens.menu".into())));
        assert_eq!(scrim.anchor, Some(UiAnchor::TopLeft));
        // Muse slot spliced verbatim, behind the popup.
        assert_eq!(muse.component, "sprite");

        // Popup: a `modal.panel` lifted onto sub-layer 1 (so the Muse can't cover it),
        // left-anchored with the hero offset.
        assert_eq!(popup.component, "cell");
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
        assert_eq!(divider.component, "cell");
        assert_eq!(divider.props.get("style"), Some(&Value::Text("modal.divider".into())));
        assert_eq!(divider.size, Some(1.0));
        // Items: the caller's buttons wrapped in a column so they carry their own gap.
        assert_eq!(items.component, "cell");
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
        assert_eq!(out.component, "screen");
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
        assert_eq!(popup.children[1].component, "cell");
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

        // Root: a full-screen overlay screen = [dim overlay, centred popup].
        assert_eq!(out.component, "screen");
        assert_eq!(p_num(&out.props, "width_frac"), Some(1.0));
        assert_eq!(out.children.len(), 2);
        let (overlay, popup) = (&out.children[0], &out.children[1]);

        // Overlay: full-screen panel wearing the scene's overlay style.
        assert_eq!(overlay.component, "cell");
        assert_eq!(p_text(&overlay.props, "style"), Some("screens.confirm"));
        assert_eq!(p_num(&overlay.props, "width_frac"), Some(1.0));

        // Popup: centred modal panel, lifted above the overlay.
        assert_eq!(popup.component, "cell");
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
        assert_eq!(divider.component, "cell");
        assert_eq!(p_text(&divider.props, "style"), Some("modal.divider"));

        // Two prop-built buttons, in order, each variant-styled with its action.
        assert_eq!(buttons.component, "cell");
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
        assert_eq!(buttons.component, "cell");
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
        // Both are styled `rtt`s, each grown to share the row, ids from the prefix.
        assert_eq!(left.component, "rtt");
        assert_eq!(right.component, "rtt");
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

    /// A `quad_rtt_view` with only a `source` expands to ONE styled `rtt` piece
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

        // One `rtt` piece — a thin holder, no children and no leftover slots.
        assert_eq!(out.component, "rtt");
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
    /// replaces the default frame, a fixed size authored STRUCTURALLY on the
    /// instance node (the only form the Lua/JSON parsers produce — both consume
    /// `width`/`height` structurally) rides `overlay_placement` onto the stage,
    /// and a literal `live` bool rides through verbatim.
    #[test]
    fn quad_rtt_view_honours_quad_id_style_and_fixed_size() {
        let reg = builtin_templates();
        let mut node = template_node("quad_rtt_view", vec![]);
        node.id = "node_id".to_string();
        node.width = Some(512.0);
        node.height = Some(512.0);
        node.props
            .insert("quad_id".to_string(), Value::Text("kiln_grid".to_string()));
        node.props.insert(
            "style".to_string(),
            Value::Text("loomforge.clip_stage".to_string()),
        );
        node.props
            .insert("source".to_string(), Value::Text("lighting".to_string()));
        node.props.insert("live".to_string(), Value::Bool(false));
        let out = expand(node, &reg);

        assert_eq!(out.component, "rtt");
        // `quad_id` wins over the node id; the explicit style overrides the default.
        assert_eq!(out.id, "kiln_grid");
        assert_eq!(text_prop(&out, "style"), Some("loomforge.clip_stage"));
        assert_eq!(text_prop(&out, "source"), Some("lighting"));
        // The instance's structural size lands on the stage via `overlay_placement`.
        assert_eq!(out.width, Some(512.0));
        assert_eq!(out.height, Some(512.0));
        // Literal liveness bool rides through as a `Bool` value.
        assert_eq!(out.props.get("live"), Some(&Value::Bool(false)));
    }
}
