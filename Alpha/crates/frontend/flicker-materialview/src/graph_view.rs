//! **The graph VIEW** — a [`MaterialGraph`] projected onto the cards and wires of a
//! [`GraphCanvas`](flicker_canvas::GraphCanvas).
//!
//! ONE projection for every host that draws a material's graph — the MaterialBench's
//! designer, and the Material Catalog's read-only inspector — so a graph reads the same
//! wherever it is shown (rule DDD070C7: one implementation, moved — never copied):
//!
//! - [`GraphCards::project`] — one card per node in EVALUATION order
//!   ([`MaterialGraph::eval_order`], the evaluator's own walk), then the Output card
//!   ([`OUTPUT_CARD`]) with a label per port; one edge per wire that resolves, each ending
//!   on its sink port's dot, inked by the selection.
//! - [`CardLabels`] — the words on the cards: a kind's name (`$mtl_kind_<id>`), the Output
//!   card's heading (`$mtl_output`) and its port labels (`$mtl_outport_<name>`), resolved
//!   through the stringtable ([`CardLabels::stringtable`]) or handed in (a test).
//! - [`with_nodes`] — the cards as the filler's borrowed [`GraphNode`]s.
//! - [`canvas_style`] — the filler's ink, read from the host's own style block.
//! - [`fit_floor`] — the readable zoom a host frames its graph at, from its own params.
//! - [`shape_pointer`] — the walker's sample for a filler's seat (the canvas's, and the lit
//!   sample's for its zoom wheel), gated the one way every host gates it.
//!
//! The projection is pure data — no GPU, no walker state — so a gate reads exactly the
//! cards a reader sees.

use std::collections::HashMap;

use flicker::render::{Rect, Vec2};
use flicker::ui::{strings, SurfacePointer, SurfaceSlot};
use flicker_canvas::{
    rect_contains, CanvasMetrics, CanvasStyle, EdgeInk, GraphEdge, GraphNode, PointerSample,
};
use flicker_texture::graph::OUTPUT_PORTS;
use flicker_texture::{MaterialGraph, NodeKind};

/// The canvas key of the Output pseudo-card. A node key no designer generates — hand
/// placement on a canvas is keyed by card key, so it must not collide with one.
pub const OUTPUT_CARD: &str = "@output";

/// The Output card's heading, as a stringtable token.
pub const OUTPUT_TOKEN: &str = "$mtl_output";

/// A node kind's name, as a stringtable token — `$mtl_kind_<id>`.
pub fn kind_token(id: &str) -> String {
    format!("$mtl_kind_{id}")
}

/// A node kind's NAME, resolved through the stringtable ([`kind_token`]) — what a card's
/// meta line, a node row and an insertion option all call the kind.
pub fn kind_name(id: &str) -> String {
    strings::resolve(&kind_token(id)).into_owned()
}

/// A node's HEADING as a list row, a source option and a read-out show it: its key (data)
/// and its kind's name — `key · Kind`, spelled once.
pub fn node_heading(key: &str, kind_name: &str) -> String {
    format!("{key} · {kind_name}")
}

/// An Output port's label, as a stringtable token — `$mtl_outport_<name>`, the port's
/// name from the ONE output table with its `.` spelled `_` (`emit.color` →
/// `$mtl_outport_emit_color`).
pub fn outport_token(name: &str) -> String {
    format!("$mtl_outport_{}", name.replace('.', "_"))
}

/// What a card stands for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Card {
    /// The node at this index of the graph's node list.
    Node(usize),
    /// The Output pseudo-card.
    Output,
}

/// Where a wire lands: one input port of a node (by key), or one of the Output's — its
/// row in [`OUTPUT_PORTS`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Sink {
    Input { node: String, port: usize },
    Output(usize),
}

impl Sink {
    /// The port the wire ends on — the dot on its sink card.
    pub fn port(&self) -> usize {
        match self {
            Sink::Input { port, .. } => *port,
            Sink::Output(port) => *port,
        }
    }
}

/// The words on the cards, resolved ONCE: each kind's name, the Output card's heading and
/// one label per Output port.
#[derive(Clone, Debug)]
pub struct CardLabels {
    kinds: HashMap<&'static str, String>,
    /// The Output card's heading.
    pub output_title: String,
    /// One label per Output port, in [`OUTPUT_PORTS`] order.
    pub output_ports: Vec<String>,
}

impl CardLabels {
    /// The stringtable's words — [`kind_name`] per kind, [`OUTPUT_TOKEN`],
    /// [`outport_token`] per port — resolved at the draw boundary's own resolver.
    pub fn stringtable() -> Self {
        Self::new(|token| strings::resolve(token).into_owned())
    }

    /// The words through `resolve`, which is asked for each TOKEN: the stringtable at
    /// runtime ([`Self::stringtable`]), anything a test hands in.
    pub fn new(resolve: impl Fn(&str) -> String) -> Self {
        Self {
            kinds: NodeKind::catalog()
                .iter()
                .map(|k| (k.id(), resolve(&kind_token(k.id()))))
                .collect(),
            output_title: resolve(OUTPUT_TOKEN),
            output_ports: OUTPUT_PORTS
                .iter()
                .map(|p| resolve(&outport_token(p.name)))
                .collect(),
        }
    }

    /// A kind's name. Every kind is in [`NodeKind::catalog`] (flicker-texture's own gate),
    /// so the id itself — visibly not a name — is only ever a new kind's that the catalog
    /// has not learned yet.
    pub fn kind_name(&self, id: &str) -> String {
        self.kinds
            .get(id)
            .cloned()
            .unwrap_or_else(|| id.to_string())
    }
}

/// A graph's cards and wires — one card per node in EVALUATION order, then the Output card;
/// one edge per wire that resolves. Index for index: `keys`, `titles`, `metas`, `ports`,
/// `port_labels`, `selected` and `cards`; `edges` with `sinks`.
#[derive(Clone, Debug, Default)]
pub struct GraphCards {
    /// Card keys (node keys, then [`OUTPUT_CARD`]) — hand placement is keyed by these.
    pub keys: Vec<String>,
    /// Card headings (a node's key; the Output card's heading).
    pub titles: Vec<String>,
    /// Each card's one meta line — a node's kind name; empty for the Output card.
    pub metas: Vec<String>,
    /// Input ports per card — the dots down a card's edge.
    pub ports: Vec<u8>,
    /// Per card, a label per port — the Output card's; a node card's is empty (its ports
    /// are named by its parameters).
    pub port_labels: Vec<Vec<String>>,
    pub selected: Vec<bool>,
    /// What each card stands for.
    pub cards: Vec<Card>,
    pub edges: Vec<GraphEdge>,
    /// Where each edge lands, index for index with `edges`.
    pub sinks: Vec<Sink>,
}

impl GraphCards {
    /// Project `graph`: its cards in evaluation order plus the Output card, an edge for
    /// every wire that resolves (a node's inputs, then the Output's ports in table order),
    /// worded by `labels`. `selected` is a card key — a node's, or [`OUTPUT_CARD`] — whose
    /// card wears the selection and whose wires are lit; `selected_edge` is a wire, by where
    /// it lands, inked as selected.
    pub fn project(
        graph: &MaterialGraph,
        labels: &CardLabels,
        selected: Option<&str>,
        selected_edge: Option<&Sink>,
    ) -> Self {
        let order = graph.eval_order();
        let mut c = Self::default();
        let mut card_of = HashMap::with_capacity(order.len());
        for &i in &order {
            let n = &graph.nodes[i];
            card_of.entry(n.key.as_str()).or_insert(c.keys.len());
            c.keys.push(n.key.clone());
            c.titles.push(n.key.clone());
            c.metas.push(labels.kind_name(n.kind.id()));
            c.ports.push(n.kind.ports().len() as u8);
            c.port_labels.push(Vec::new());
            c.selected.push(selected == Some(n.key.as_str()));
            c.cards.push(Card::Node(i));
        }
        let out_card = c.keys.len();
        c.keys.push(OUTPUT_CARD.to_string());
        c.titles.push(labels.output_title.clone());
        c.metas.push(String::new());
        c.ports.push(OUTPUT_PORTS.len() as u8);
        c.port_labels.push(labels.output_ports.clone());
        c.selected.push(selected == Some(OUTPUT_CARD));
        c.cards.push(Card::Output);

        let sel_card = selected.and_then(|k| {
            if k == OUTPUT_CARD {
                Some(out_card)
            } else {
                card_of.get(k).copied()
            }
        });
        // Every edge ENDS ON ITS SINK PORT's dot — two wires between the same two cards are
        // two lines, each the one picked near its own port.
        let mut edge = |from: usize, to: usize, sink: Sink| {
            let ink = if selected_edge == Some(&sink) {
                EdgeInk::Selected
            } else if sel_card.is_some_and(|s| s == from || s == to) {
                EdgeInk::Lit
            } else {
                EdgeInk::Idle
            };
            c.edges.push(GraphEdge {
                from,
                to,
                ink,
                port: Some(sink.port()),
            });
            c.sinks.push(sink);
        };
        for &i in &order {
            let n = &graph.nodes[i];
            let to = card_of[n.key.as_str()];
            for (port, src) in n.inputs.iter().enumerate() {
                if let Some(&from) = src.as_deref().and_then(|k| card_of.get(k)) {
                    let node = n.key.clone();
                    edge(from, to, Sink::Input { node, port });
                }
            }
        }
        for (port, src) in graph.output.sources().into_iter().enumerate() {
            if let Some(&from) = src.and_then(|k| card_of.get(k)) {
                edge(from, out_card, Sink::Output(port));
            }
        }
        c
    }

    /// The card keys as the filler's `&[&str]`.
    pub fn key_refs(&self) -> Vec<&str> {
        self.keys.iter().map(String::as_str).collect()
    }
}

/// The cards as the filler's borrowed [`GraphNode`]s, handed to `f` — the shape a host's
/// layout, pick and draw calls all take.
pub fn with_nodes<R>(cards: &GraphCards, f: impl FnOnce(&[GraphNode<'_>]) -> R) -> R {
    let lines: Vec<[&str; 1]> = cards.metas.iter().map(|m| [m.as_str()]).collect();
    let labels: Vec<Vec<&str>> = cards
        .port_labels
        .iter()
        .map(|l| l.iter().map(String::as_str).collect())
        .collect();
    let nodes: Vec<GraphNode<'_>> = (0..cards.keys.len())
        .map(|i| GraphNode {
            title: &cards.titles[i],
            meta: if cards.metas[i].is_empty() {
                &[]
            } else {
                &lines[i]
            },
            selected: cards.selected[i],
            icon: false,
            ports: cards.ports[i],
            port_labels: &labels[i],
        })
        .collect();
    f(&nodes)
}

/// Every [`CanvasStyle`] slot, by the name a host's style block authors it under.
pub const CANVAS_INK: [&str; 15] = [
    "bg",
    "edge",
    "edge_lit",
    "card_fill_top",
    "card_fill_bot",
    "card_border",
    "card_border_selected",
    "label",
    "label_selected",
    "meta",
    "icon_top",
    "icon_bot",
    "icon_border",
    "port",
    "link",
];

/// The canvas's ink from a host's OWN style block: every [`CANVAS_INK`] slot read at
/// `<prefix>.<slot>` of the token-resolved `styles` (each a `$token` the host authored —
/// colour belongs to the consumer). A slot that resolves to no rgba is LOUD and
/// transparent: visibly unset, never a guess.
pub fn canvas_style(styles: &serde_json::Value, prefix: &str) -> CanvasStyle {
    let ink = |slot: &str| {
        let path = format!("{prefix}.{slot}");
        let value = path.split('.').try_fold(styles, |v, k| v.get(k));
        match value.and_then(serde_json::Value::as_array) {
            Some(a) if a.len() >= 4 => std::array::from_fn(|i| a[i].as_f64().unwrap_or(0.0) as f32),
            _ => {
                tracing::warn!("graph view: style `{path}` is not an rgba array — unset ink");
                [0.0; 4]
            }
        }
    };
    CanvasStyle {
        bg: ink("bg"),
        edge: ink("edge"),
        edge_lit: ink("edge_lit"),
        card_fill_top: ink("card_fill_top"),
        card_fill_bot: ink("card_fill_bot"),
        card_border: ink("card_border"),
        card_border_selected: ink("card_border_selected"),
        label: ink("label"),
        label_selected: ink("label_selected"),
        meta: ink("meta"),
        icon_top: ink("icon_top"),
        icon_bot: ink("icon_bot"),
        icon_border: ink("icon_border"),
        port: ink("port"),
        link: ink("link"),
    }
}

/// The canvas's READABLE zoom floor a host authors — `params.<key>` of its `file`: a graph is
/// framed whole only while that takes no smaller a zoom than this; past it the camera frames
/// the host's focus at the floor (`GraphCanvas::fit_legible`). A missing or malformed value
/// is LOUD and falls back to the canvas's own zoom floor (a fit as small as it likes).
pub fn fit_floor(
    params: &serde_json::Map<String, serde_json::Value>,
    key: &str,
    file: &str,
) -> f32 {
    match params.get(key).and_then(serde_json::Value::as_f64) {
        Some(v) if v.is_finite() && v > 0.0 => v as f32,
        authored => {
            let fallback = CanvasMetrics::default().zoom_min;
            tracing::error!(
                "{file} authors no usable `params.{key}` ({authored:?}) — a fit may zoom out \
                 to the canvas floor {fallback}"
            );
            fallback
        }
    }
}

/// A walker slot's rect — the seat a canvas is laid out and picked in.
pub fn seat_of(slot: &SurfaceSlot) -> Rect {
    Rect {
        pos: Vec2::new(slot.x, slot.y),
        size: Vec2::new(slot.w, slot.h),
    }
}

/// The walker's pointer sample for a filler seated at `seat` — a graph canvas, or a lit
/// sample whose wheel zooms it — shaped the one way every host gates it, and returned with
/// whether the cursor is over the seat at all (an open menu notwithstanding: a host abandons
/// a link released off the seat by it).
///
/// The walker hands its surface sample while the cursor is over the canvas with nothing
/// painted over it claiming, or while a press that began there is held — the panel AROUND a
/// canvas claims its rect (`hud_hit` is true all over it), and is rightly not what occludes
/// it. What does is an open menu, which the walker answers itself (`popup_hit`: the cursor
/// is in the menu, or this frame's click was spent closing it) — no press, wheel notch or
/// pan of that frame is the canvas's. The press edge is the LEFT button's (the walker's is
/// either button's), and middle-drag pans only while over the seat.
pub fn shape_pointer(
    pointer: Option<&SurfacePointer>,
    seat: Rect,
    middle: bool,
    popup_hit: bool,
) -> (PointerSample, bool) {
    let mut sample = pointer.map(PointerSample::from).unwrap_or_default();
    let over_seat = pointer.is_some() && rect_contains(seat, sample.cursor);
    sample.inside = over_seat && !popup_hit;
    sample.pressed &= sample.left && !popup_hit;
    if !sample.inside {
        sample.wheel = 0.0;
    }
    sample.middle = sample.inside && middle;
    (sample, over_seat)
}

#[cfg(test)]
mod tests {
    use super::*;
    use flicker_texture::load_material_json;

    /// A staged glow's shape (slot 248's): four nodes — a worley `cells`, the `veins` band
    /// and the `body` ramp it feeds, and the `glow` palette — wired to base, height and the
    /// glow's colour and mask; the three scalars rest at constants.
    const GLOW: &str = r#"{"version": 2, "id": "glow_red", "name": "Glow Red", "material": 248,
        "seed": 248, "nodes": [
        {"key": "cells", "kind": "noise", "source": "worley", "scale": 5, "octaves": 1,
         "lacunarity": 2.0, "gain": 0.5, "warp": 0.5, "salt": 1, "contrast": 1.0,
         "invert": false, "inputs": []},
        {"key": "veins", "kind": "band", "at": 0.55, "width": 1.0, "soft": 0.15,
         "inputs": ["cells"]},
        {"key": "body", "kind": "ramp", "ramp": {"stops": [
            {"at": 0.0, "color": [0.065, 0.015, 0.015]},
            {"at": 1.0, "color": [0.25, 0.05, 0.05]}]}, "inputs": ["cells"]},
        {"key": "glow", "kind": "palette", "rgb": [1.0, 0.0, 0.0], "inputs": []}],
        "output": {"base": "body", "height": "cells", "relief": 0.5, "roughness": 0.6,
        "metalness": 0.0, "ao": 1.0,
        "emit": {"color": "glow", "mask": "veins", "strength": 1.0}}}"#;

    /// Words that say which token they came from.
    fn echo() -> CardLabels {
        CardLabels::new(|token| format!("<{token}>"))
    }

    fn glow() -> MaterialGraph {
        load_material_json(GLOW).expect("the glow document loads")
    }

    fn wires(c: &GraphCards) -> Vec<(usize, usize, Option<usize>)> {
        c.edges.iter().map(|e| (e.from, e.to, e.port)).collect()
    }

    /// THE GLOW: four nodes project to FIVE cards — the nodes in evaluation order, the
    /// Output card last with a label per port — and SIX wires, each ending on its port.
    #[test]
    fn a_four_node_glow_projects_to_five_cards_and_its_wires() {
        let c = GraphCards::project(&glow(), &echo(), None, None);
        assert_eq!(c.keys, ["cells", "veins", "body", "glow", OUTPUT_CARD]);
        assert_eq!(
            c.cards,
            [
                Card::Node(0),
                Card::Node(1),
                Card::Node(2),
                Card::Node(3),
                Card::Output
            ]
        );
        assert_eq!(c.titles[..4], ["cells", "veins", "body", "glow"]);
        assert_eq!(c.titles[4], "<$mtl_output>");
        assert_eq!(
            c.metas,
            [
                "<$mtl_kind_noise>",
                "<$mtl_kind_band>",
                "<$mtl_kind_ramp>",
                "<$mtl_kind_palette>",
                ""
            ]
        );
        assert_eq!(c.ports, [0, 1, 1, 0, 7]);
        assert!(c.port_labels[..4].iter().all(Vec::is_empty));
        assert_eq!(
            c.port_labels[4],
            [
                "<$mtl_outport_base>",
                "<$mtl_outport_height>",
                "<$mtl_outport_roughness>",
                "<$mtl_outport_metalness>",
                "<$mtl_outport_ao>",
                "<$mtl_outport_emit_color>",
                "<$mtl_outport_emit_mask>"
            ]
        );
        // The nodes' inputs in evaluation order, then the Output's ports in table order:
        // veins ← cells, body ← cells; base ← body, height ← cells, emit.color ← glow,
        // emit.mask ← veins. The three constant scalars are no wires.
        assert_eq!(
            wires(&c),
            [
                (0, 1, Some(0)),
                (0, 2, Some(0)),
                (2, 4, Some(0)),
                (0, 4, Some(1)),
                (3, 4, Some(5)),
                (1, 4, Some(6)),
            ]
        );
        assert_eq!(
            c.sinks[0],
            Sink::Input {
                node: "veins".into(),
                port: 0
            }
        );
        assert_eq!(c.sinks[4], Sink::Output(5));
        assert!(
            c.selected.iter().all(|s| !s),
            "nothing is picked until a reader picks"
        );
        assert!(c.edges.iter().all(|e| e.ink == EdgeInk::Idle));
    }

    /// Cards follow the EVALUATION order, not the list's — a node after everything that
    /// feeds it — and the Output card is LAST, an empty graph's only card.
    #[test]
    fn cards_follow_evaluation_order_and_the_output_comes_last() {
        let mut g = glow();
        // List order glow, body, veins, cells: the feeds must still come first.
        g.nodes.reverse();
        let c = GraphCards::project(&g, &echo(), None, None);
        assert_eq!(c.keys, ["glow", "cells", "body", "veins", OUTPUT_CARD]);
        assert_eq!(
            c.cards,
            [
                Card::Node(0),
                Card::Node(3),
                Card::Node(1),
                Card::Node(2),
                Card::Output
            ]
        );
        assert_eq!(
            c.edges.len(),
            6,
            "the same six wires, whatever the list order"
        );
        g.nodes.clear();
        let empty = GraphCards::project(&g, &echo(), None, None);
        assert_eq!(empty.keys, [OUTPUT_CARD]);
        assert!(empty.edges.is_empty(), "no node, no wire");
    }

    /// A picked card wears the selection and lights its wires; a picked wire is inked
    /// selected; the Output card can be picked like any other.
    #[test]
    fn the_selection_inks_its_card_and_its_wires() {
        let g = glow();
        let ink = |c: &GraphCards| c.edges.iter().map(|e| e.ink).collect::<Vec<_>>();
        let (idle, lit, sel) = (EdgeInk::Idle, EdgeInk::Lit, EdgeInk::Selected);
        let c = GraphCards::project(&g, &echo(), Some("veins"), None);
        assert_eq!(c.selected, [false, true, false, false, false]);
        assert_eq!(ink(&c), [lit, idle, idle, idle, idle, lit]);
        let c = GraphCards::project(&g, &echo(), Some(OUTPUT_CARD), Some(&Sink::Output(0)));
        assert_eq!(c.selected, [false, false, false, false, true]);
        assert_eq!(ink(&c), [idle, idle, sel, lit, lit, lit]);
        let c = GraphCards::project(&g, &echo(), Some("gone"), None);
        assert!(
            c.selected.iter().all(|s| !s),
            "a key no card has picks nothing"
        );
    }

    /// The SHIPPED words: every kind, the Output heading and every Output port resolve
    /// through the stringtable to a name, never a raw token.
    #[test]
    fn the_stringtable_names_every_kind_and_port() {
        let table = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../content/data/stringtable.json"
        ))
        .expect("the stringtable reads");
        strings::load_str(&table, "en-us");
        let labels = CardLabels::stringtable();
        for kind in NodeKind::catalog() {
            let name = labels.kind_name(kind.id());
            assert!(
                !name.starts_with('$') && name != kind.id(),
                "{}: {name}",
                kind.id()
            );
        }
        assert!(
            !labels.output_title.starts_with('$'),
            "{}",
            labels.output_title
        );
        assert_eq!(labels.output_ports.len(), OUTPUT_PORTS.len());
        for (port, label) in OUTPUT_PORTS.iter().zip(&labels.output_ports) {
            assert!(!label.starts_with('$'), "{}: {label}", port.name);
        }
    }

    /// The canvas ink is the host's block, slot for slot; a slot the block lacks is unset
    /// (transparent), never a guess.
    #[test]
    fn the_ink_is_read_slot_for_slot_from_the_hosts_block() {
        let block: serde_json::Map<String, serde_json::Value> = CANVAS_INK
            .iter()
            .enumerate()
            .map(|(i, slot)| {
                (
                    slot.to_string(),
                    serde_json::json!([i as f64, 0.0, 0.0, 1.0]),
                )
            })
            .collect();
        let styles = serde_json::json!({ "host": { "canvas": block } });
        let s = canvas_style(&styles, "host.canvas");
        assert_eq!(s.bg, [0.0, 0.0, 0.0, 1.0]);
        assert_eq!(s.card_border_selected, [6.0, 0.0, 0.0, 1.0]);
        assert_eq!(s.link, [14.0, 0.0, 0.0, 1.0]);
        let unset = canvas_style(&styles, "host.missing");
        assert_eq!(unset.edge, [0.0; 4]);
    }

    /// The fit floor is the host's authored number; anything else falls back to the canvas's
    /// own floor.
    #[test]
    fn the_fit_floor_is_the_hosts_number_or_the_canvas_floor() {
        let params = |v: serde_json::Value| {
            serde_json::json!({ "canvas_fit_floor": v })
                .as_object()
                .cloned()
                .expect("an object")
        };
        let floor = CanvasMetrics::default().zoom_min;
        assert_eq!(
            fit_floor(&params(serde_json::json!(0.75)), "canvas_fit_floor", "t"),
            0.75
        );
        for bad in [
            serde_json::json!(0.0),
            serde_json::json!(-1.0),
            serde_json::json!("x"),
        ] {
            assert_eq!(fit_floor(&params(bad), "canvas_fit_floor", "t"), floor);
        }
        assert_eq!(
            fit_floor(&serde_json::Map::new(), "canvas_fit_floor", "t"),
            floor
        );
    }

    /// The pointer gate: a press is the canvas's only when it is the LEFT button's, over the
    /// seat, and not spent on an open menu; the wheel and a middle-drag only over the seat.
    #[test]
    fn the_pointer_is_the_canvas_only_over_its_seat_and_never_under_a_menu() {
        let seat = Rect {
            pos: Vec2::new(10.0, 10.0),
            size: Vec2::new(100.0, 50.0),
        };
        let at = |x: f32, left: bool| SurfacePointer {
            id: "canvas".into(),
            root: false,
            cursor: Vec2::new(x, 20.0),
            local: Vec2::new(x - 10.0, 10.0),
            delta: Vec2::new(0.0, 0.0),
            left,
            right: !left,
            pressed: true,
            wheel: 1.0,
            captured: false,
            rect: seat,
        };
        let (s, over) = shape_pointer(Some(&at(50.0, true)), seat, true, false);
        assert!(over && s.inside && s.pressed && s.middle);
        assert_eq!(s.wheel, 1.0);
        let (s, _) = shape_pointer(Some(&at(50.0, false)), seat, false, false);
        assert!(!s.pressed, "a right press picks nothing");
        let (s, over) = shape_pointer(Some(&at(50.0, true)), seat, true, true);
        assert!(over && !s.inside && !s.pressed && !s.middle && s.wheel == 0.0);
        let (s, over) = shape_pointer(Some(&at(500.0, true)), seat, true, false);
        assert!(!over && !s.inside && s.wheel == 0.0 && !s.middle);
        let (s, over) = shape_pointer(None, seat, true, false);
        assert!(!over && !s.inside && !s.pressed);
    }
}
