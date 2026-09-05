//! **`GraphCanvas`** — the node-graph editor surface.
//!
//! Cards on a pannable, zoomable plane, joined by edges that stop on the cards'
//! borders (and loop over the top for a self-edge). Every gesture an author needs on
//! such a graph lives here — wheel zoom anchored under the cursor, middle-drag pan,
//! card drag, link rubber-band, and the three picks ([`GraphCanvas::card_at`],
//! [`GraphCanvas::edge_at`], [`GraphCanvas::port_at`]) — while what a gesture MEANS
//! stays with the consumer: this filler reports "a press landed on card 3", never
//! "delete state 3".
//!
//! **Hand placement is EDITOR-SIDE.** Where a card sits is the author's arrangement
//! of their workspace, not a fact about the graph, so it is never written back into
//! the consumer's document; and it is keyed by the node's stable KEY rather than its
//! index, so it survives nodes being added, removed or reordered underneath it.

use std::collections::HashMap;

use flicker::render::{Rect, Vec2};
use flicker::script::{FontRole, HudCommand, TextAlign};

use crate::{rect_contains, PointerSample};

/// Card geometry and interaction distances. Every field is a design value with a
/// working [`Default`]; a consumer overrides only what its own design differs on.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CanvasMetrics {
    /// Card size at zoom 1.
    pub card: Vec2,
    /// Margin between a card's border and the icon inside it.
    pub card_pad: f32,
    /// The square media slot on a card's left (a live doll, a thumbnail).
    pub icon: f32,
    /// Default-grid gaps between cards.
    pub gap: Vec2,
    /// Grid inset from the canvas's top-left.
    pub pad: f32,
    /// Card corner radius at zoom 1.
    pub radius: f32,
    /// Card border thickness, unselected / selected.
    pub border: f32,
    pub border_selected: f32,
    /// Title line: size at zoom 1, and its baseline down from the card's top.
    pub title_size: f32,
    pub title_y: f32,
    /// Meta lines: size, first baseline, and the step between them.
    pub meta_size: f32,
    pub meta_y: f32,
    pub meta_step: f32,
    /// Zoom floor and ceiling — far enough out to see a large graph whole, far
    /// enough in to read a card's smallest line.
    pub zoom_min: f32,
    pub zoom_max: f32,
    /// Zoom factor per wheel notch.
    pub zoom_step: f32,
    /// How near an edge the pointer must be to grab it, in SCREEN pixels. Edges are
    /// 1–3px lines, so picking needs a tolerance far wider than what is drawn.
    pub edge_grab: f32,
    /// How far a self-edge's loop arc rises above its card, at zoom 1.
    pub self_loop_lift: f32,
    /// Edge widths: idle, lit (an end is selected), and picked.
    pub edge_width: f32,
    pub edge_width_lit: f32,
    pub edge_width_selected: f32,
    /// Link rubber-band width.
    pub link_width: f32,
    /// Port dot diameter at zoom 1, and how near one must be clicked.
    pub port: f32,
    pub port_grab: f32,
}

impl Default for CanvasMetrics {
    fn default() -> Self {
        Self {
            card: Vec2::new(220.0, 88.0),
            card_pad: 10.0,
            icon: 68.0,
            gap: Vec2::new(80.0, 42.0),
            pad: 28.0,
            radius: 5.0,
            border: 1.0,
            border_selected: 2.0,
            title_size: 17.0,
            title_y: 12.0,
            meta_size: 11.0,
            meta_y: 38.0,
            meta_step: 18.0,
            zoom_min: 0.35,
            zoom_max: 2.0,
            zoom_step: 0.12,
            edge_grab: 7.0,
            self_loop_lift: 20.0,
            edge_width: 1.4,
            edge_width_lit: 2.4,
            edge_width_selected: 3.0,
            link_width: 2.0,
            port: 7.0,
            port_grab: 7.0,
        }
    }
}

/// Every colour the canvas paints with — filled by the CONSUMER from its theme
/// tokens. There are no colour literals in this crate; [`CanvasStyle::blank`] leaves
/// every slot transparent, which is the absence of a colour rather than a choice.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CanvasStyle {
    /// The canvas ground, painted over the whole seat.
    pub bg: [f32; 4],
    pub edge: [f32; 4],
    /// An edge with a selected end, and the picked edge itself.
    pub edge_lit: [f32; 4],
    pub card_fill_top: [f32; 4],
    pub card_fill_bot: [f32; 4],
    pub card_border: [f32; 4],
    pub card_border_selected: [f32; 4],
    pub label: [f32; 4],
    pub label_selected: [f32; 4],
    pub meta: [f32; 4],
    /// The media slot's backdrop, drawn under whatever the consumer composites into
    /// it — so a card reads correctly before its image exists.
    pub icon_top: [f32; 4],
    pub icon_bot: [f32; 4],
    pub icon_border: [f32; 4],
    pub port: [f32; 4],
    /// The in-flight link rubber-band.
    pub link: [f32; 4],
}

impl CanvasStyle {
    /// Every slot transparent. A canvas drawn with this paints nothing — the loud
    /// symptom of a consumer that has not filled its style from the theme.
    pub fn blank() -> Self {
        Self::default()
    }
}

/// What the left button MEANS on this canvas — the consumer's active tool, reduced
/// to the three gestures the canvas itself performs.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CanvasMode {
    /// A press on a card picks it up and drags it (editor-side layout).
    #[default]
    Select,
    /// A press on a card starts a link; the release names the target.
    Link,
    /// Presses report what they hit and nothing else — for tools that create or
    /// destroy rather than move (add / delete / inspect).
    Inspect,
}

/// One node the consumer wants laid out and drawn this frame.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct GraphNode<'a> {
    /// The card's heading.
    pub title: &'a str,
    /// Smaller lines under it, drawn in the meta colour.
    pub meta: &'a [&'a str],
    pub selected: bool,
    /// Reserve and back the square media slot on the card's left.
    pub icon: bool,
    /// Connector stubs on the card's right edge. `0` = none, and the whole card is
    /// the connector (which is what a state-machine graph wants).
    pub ports: u8,
}

/// How an edge should read — the consumer's call, since only it knows what
/// "selected" means in its document.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum EdgeInk {
    #[default]
    Idle,
    /// One of its ends is selected.
    Lit,
    /// This edge is the selected one.
    Selected,
}

/// One edge, by node index into the same slice `layout` was given.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct GraphEdge {
    pub from: usize,
    pub to: usize,
    pub ink: EdgeInk,
}

/// A press the canvas resolved. A card wins over an edge (cards are drawn on top),
/// and `local` is where the press landed in canvas-local coordinates so a consumer
/// that places something there does not have to invert the view itself.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Press {
    pub card: Option<usize>,
    pub edge: Option<usize>,
    pub local: Vec2,
}

/// A completed link drag. `to` is `None` when the release landed on empty canvas,
/// and may equal `from` — refusing a self-link is the consumer's rule, not this
/// filler's (a state machine allows one; a dependency graph does not).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Link {
    pub from: usize,
    pub to: Option<usize>,
}

/// What one [`GraphCanvas::pointer`] pass produced. Both can land in the same frame
/// (a press and a release of an earlier drag), so neither hides the other.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CanvasEvents {
    pub pressed: Option<Press>,
    pub linked: Option<Link>,
}

/// How the author has arranged their workspace: where the graph is panned and scaled
/// to, and any cards they have placed by hand.
///
/// **None of this belongs to the consumer's document.** Hand positions are keyed by
/// node KEY rather than index, so they survive nodes being added, removed or
/// reordered underneath them.
#[derive(Clone, Debug, Default)]
pub struct View {
    pub pan: Vec2,
    pub zoom: f32,
    pub placed: HashMap<String, Vec2>,
}

impl View {
    /// A fresh view: unpanned, unzoomed, nothing placed by hand.
    pub fn new() -> Self {
        Self {
            pan: Vec2::ZERO,
            zoom: 1.0,
            placed: HashMap::new(),
        }
    }

    /// Canvas-local point → screen pixels.
    pub fn to_screen(&self, area: Rect, local: Vec2) -> Vec2 {
        area.pos + local * self.zoom + self.pan
    }

    /// Screen pixels → canvas-local point. The inverse of [`View::to_screen`].
    pub fn to_local(&self, area: Rect, screen: Vec2) -> Vec2 {
        (screen - area.pos - self.pan) / self.zoom
    }

    /// Zoom about a fixed screen point — the cursor stays over the same part of the
    /// graph, which is what makes wheel-zoom feel anchored instead of lurching.
    pub fn zoom_at(&mut self, area: Rect, anchor: Vec2, factor: f32, min: f32, max: f32) {
        let before = self.to_local(area, anchor);
        self.zoom = (self.zoom * factor).clamp(min, max);
        let after = self.to_local(area, anchor);
        // Re-pan so `anchor` maps back to the same local point it did before.
        self.pan += (after - before) * self.zoom;
    }
}

/// A laid-out edge: one segment for a straight run, three for a self-loop arc over
/// the card (a straight line to itself would vanish inside it).
#[derive(Clone, Copy, Debug)]
struct EdgeGeom {
    /// Index into the consumer's edge slice — what [`GraphCanvas::edge_at`] returns,
    /// so an edge that could not be laid out never shifts the others' identities.
    id: usize,
    segs: [(Vec2, Vec2); 3],
    n: usize,
}

impl EdgeGeom {
    /// The segment the pointer picks against — the middle one, which is the whole
    /// edge for a straight run and the top bar of a self-loop.
    fn pick(&self) -> (Vec2, Vec2) {
        self.segs[self.n / 2]
    }
}

/// **The node-graph surface filler.** Seat it in the walker-reserved rect, lay out
/// this frame's keys and edges, hand it the pointer sample, then draw.
pub struct GraphCanvas {
    metrics: CanvasMetrics,
    /// The walker-reserved rect this canvas fills.
    area: Rect,
    view: View,
    /// This frame's card rects — the ONE geometry both picking and drawing use.
    cards: Vec<Rect>,
    edges: Vec<EdgeGeom>,
    mode: CanvasMode,
    /// Card being dragged, with the grab offset in canvas-local space so it moves
    /// under the cursor rather than snapping its corner there.
    drag: Option<(usize, Vec2)>,
    /// Cursor position when the current pan began.
    pan_from: Option<Vec2>,
    /// While a link drag is in flight: the source card.
    link_from: Option<usize>,
    cursor: Vec2,
    /// Last frame's held state, so the release edge is derived here rather than
    /// trusted to the consumer.
    prev_left: bool,
}

impl GraphCanvas {
    pub fn new(metrics: CanvasMetrics) -> Self {
        Self {
            metrics,
            area: Rect {
                pos: Vec2::ZERO,
                size: Vec2::ZERO,
            },
            view: View::new(),
            cards: Vec::new(),
            edges: Vec::new(),
            mode: CanvasMode::default(),
            drag: None,
            pan_from: None,
            link_from: None,
            cursor: Vec2::ZERO,
            prev_left: false,
        }
    }

    /// Seat the canvas in the rect the walker reserved for it this frame. An
    /// unseated canvas (a zero rect, which is what an off-screen surface reserves)
    /// lays out nothing and picks nothing.
    pub fn seat(&mut self, area: Rect) {
        self.area = area;
    }

    pub fn area(&self) -> Rect {
        self.area
    }

    pub fn metrics(&self) -> &CanvasMetrics {
        &self.metrics
    }

    /// What the left button means on this canvas — set from the consumer's tool.
    /// Changing it abandons whatever gesture was in flight, rather than letting a
    /// link started with one tool complete under another.
    pub fn set_mode(&mut self, mode: CanvasMode) {
        if self.mode != mode {
            self.cancel();
        }
        self.mode = mode;
    }

    /// **Abandon the gesture in flight.** A consumer calls this when the canvas stops
    /// being driven — its page was left, its surface went off screen — because a drag
    /// whose release lands somewhere this filler never saw would otherwise fire that
    /// release the next time it IS driven, weaving a link the author never drew.
    pub fn cancel(&mut self) {
        self.drag = None;
        self.link_from = None;
        self.pan_from = None;
        self.prev_left = false;
    }

    pub fn view(&self) -> &View {
        &self.view
    }

    pub fn zoom(&self) -> f32 {
        self.view.zoom
    }

    /// Forget the pan, the zoom and every hand placement — a new document is a new
    /// workspace, not the previous one's camera over different cards.
    pub fn reset_view(&mut self) {
        self.view = View::new();
        self.cancel();
    }

    /// The card the link drag started from, for a consumer that wants to say so.
    pub fn linking(&self) -> Option<usize> {
        self.link_from
    }

    /// This frame's card rects, in the order the keys were given.
    pub fn cards(&self) -> &[Rect] {
        &self.cards
    }

    /// **Lay out this frame's graph.** `keys` are the nodes' stable identities (hand
    /// placement is keyed by these); `edges` index into the same order.
    ///
    /// Called every frame: it is one pass over the nodes and one over the edges, and
    /// it is what guarantees that what the pointer picks is what was drawn.
    pub fn layout(&mut self, keys: &[&str], edges: &[GraphEdge]) {
        let m = &self.metrics;
        let cols = self.columns();
        let size = m.card * self.view.zoom;
        self.cards.clear();
        self.cards.extend(keys.iter().enumerate().map(|(i, key)| {
            Rect {
                pos: self.view.to_screen(
                    self.area,
                    self.view
                        .placed
                        .get(*key)
                        .copied()
                        .unwrap_or_else(|| grid_slot(i, cols, m)),
                ),
                size,
            }
        }));

        self.edges.clear();
        for (id, e) in edges.iter().enumerate() {
            let (Some(&from), Some(&to)) = (self.cards.get(e.from), self.cards.get(e.to)) else {
                continue;
            };
            self.edges.push(if e.from == e.to {
                // Self-edge: a loop arc over the card's top edge.
                let lift = m.self_loop_lift * self.view.zoom;
                let (x0, x1) = (
                    from.pos.x + from.size.x * 0.35,
                    from.pos.x + from.size.x * 0.65,
                );
                let (y0, y1) = (from.pos.y, from.pos.y - lift);
                EdgeGeom {
                    id,
                    segs: [
                        (Vec2::new(x1, y0), Vec2::new(x1, y1)),
                        (Vec2::new(x1, y1), Vec2::new(x0, y1)),
                        (Vec2::new(x0, y1), Vec2::new(x0, y0)),
                    ],
                    n: 3,
                }
            } else {
                let (p, q) = edge_points(from, to);
                let zero = (Vec2::ZERO, Vec2::ZERO);
                EdgeGeom {
                    id,
                    segs: [(p, q), zero, zero],
                    n: 1,
                }
            });
        }
    }

    /// Columns in the default grid. Derived from the UNZOOMED area, so zooming moves
    /// the camera over a fixed arrangement instead of reflowing it under the author.
    fn columns(&self) -> usize {
        let m = &self.metrics;
        let usable = (self.area.size.x - m.pad * 2.0).max(m.card.x);
        (((usable + m.gap.x) / (m.card.x + m.gap.x)).floor() as usize).max(1)
    }

    /// The media slot's rect inside card `i` — the consumer draws or composites its
    /// image from exactly this, so the backdrop this filler paints and the picture
    /// laid over it cannot drift apart. Both scale with the zoom, like everything
    /// else on a card.
    pub fn icon_rect(&self, i: usize) -> Option<Rect> {
        let c = *self.cards.get(i)?;
        let z = self.view.zoom;
        Some(Rect {
            pos: c.pos + Vec2::splat(self.metrics.card_pad * z),
            size: Vec2::splat(self.metrics.icon * z),
        })
    }

    /// Where a card's text column starts, measured from the card's left edge.
    pub fn text_x(&self) -> f32 {
        (self.metrics.card_pad * 2.0 + self.metrics.icon) * self.view.zoom
    }

    /// Topmost card under `p` (last wins, matching draw order).
    pub fn card_at(&self, p: Vec2) -> Option<usize> {
        self.cards
            .iter()
            .enumerate()
            .rev()
            .find(|(_, c)| rect_contains(**c, p))
            .map(|(i, _)| i)
    }

    /// The edge nearest `p` within the grab radius, as an index into the edge slice
    /// the last [`GraphCanvas::layout`] was given. Nearest wins rather than first, so
    /// edges running close together pick the one actually under the cursor.
    pub fn edge_at(&self, p: Vec2) -> Option<usize> {
        self.edges
            .iter()
            .map(|e| {
                let (a, b) = e.pick();
                (e.id, dist_to_segment(p, a, b))
            })
            .filter(|(_, d)| *d <= self.metrics.edge_grab)
            .min_by(|x, y| x.1.total_cmp(&y.1))
            .map(|(id, _)| id)
    }

    /// The port under `p` as `(card, port)`. Ports are evenly spaced down a card's
    /// RIGHT edge; a node that declares none has no ports to hit, and its whole card
    /// is the connector instead.
    pub fn port_at(&self, p: Vec2, nodes: &[GraphNode]) -> Option<(usize, usize)> {
        let grab = self.metrics.port_grab * self.view.zoom;
        nodes
            .iter()
            .enumerate()
            .flat_map(|(i, n)| (0..n.ports as usize).map(move |k| (i, k)))
            .filter_map(|(i, k)| {
                let c = self.port_center(i, k, nodes)?;
                let d = (p - c).length();
                (d <= grab).then_some((i, k, d))
            })
            .min_by(|a, b| a.2.total_cmp(&b.2))
            .map(|(i, k, _)| (i, k))
    }

    /// The centre of card `i`'s port `k`, in screen pixels.
    pub fn port_center(&self, i: usize, k: usize, nodes: &[GraphNode]) -> Option<Vec2> {
        let c = *self.cards.get(i)?;
        let n = nodes.get(i)?.ports as usize;
        if k >= n {
            return None;
        }
        Some(Vec2::new(
            c.pos.x + c.size.x,
            c.pos.y + c.size.y * (k as f32 + 1.0) / (n as f32 + 1.0),
        ))
    }

    /// **Apply this frame's pointer sample.** `keys` must be the same slice the
    /// matching [`GraphCanvas::layout`] was given — a card drag writes its placement
    /// under the node's own key.
    ///
    /// Gesture order matches the order an author experiences them: navigate (zoom,
    /// pan), then continue whatever is already in flight (a card drag), then this
    /// frame's press, then this frame's release.
    pub fn pointer(&mut self, sample: &PointerSample, keys: &[&str]) -> CanvasEvents {
        let mut out = CanvasEvents::default();
        self.cursor = sample.cursor;

        // The wheel means "more of what is under the cursor": zoom about the pointer,
        // and only while the pointer is actually over the graph.
        if sample.wheel != 0.0 && sample.inside {
            let factor = 1.0 + sample.wheel * self.metrics.zoom_step;
            self.view.zoom_at(
                self.area,
                sample.cursor,
                factor,
                self.metrics.zoom_min,
                self.metrics.zoom_max,
            );
        }

        // Middle-drag pans whatever the left button currently means, so panning never
        // competes with the active tool.
        if sample.middle {
            if let Some(from) = self.pan_from {
                self.view.pan += sample.cursor - from;
            }
            self.pan_from = Some(sample.cursor);
        } else {
            self.pan_from = None;
        }

        // Move a grabbed card. Stored by KEY, editor-side — the consumer's document
        // has no notion of where a node sits on screen.
        if let Some((i, grab)) = self.drag {
            if sample.left {
                if let Some(key) = keys.get(i) {
                    let local = self.view.to_local(self.area, sample.cursor) - grab;
                    self.view.placed.insert((*key).to_string(), local);
                }
            } else {
                self.drag = None;
            }
        }

        if sample.pressed {
            let card = self.card_at(sample.cursor);
            // Cards are on top, so an edge is only picked where no card is.
            let edge = match card {
                Some(_) => None,
                None => self.edge_at(sample.cursor),
            };
            match (self.mode, card) {
                (CanvasMode::Select, Some(i)) => {
                    // Grab it where it was clicked, so it tracks the pointer instead
                    // of snapping its corner under the cursor.
                    let grab = self.view.to_local(self.area, sample.cursor)
                        - self.view.to_local(self.area, self.cards[i].pos);
                    self.drag = Some((i, grab));
                }
                (CanvasMode::Link, Some(i)) => self.link_from = Some(i),
                _ => {}
            }
            out.pressed = Some(Press {
                card,
                edge,
                local: self.view.to_local(self.area, sample.cursor),
            });
        }

        // The release edge, latched here so a consumer cannot strand a link mid-air.
        if self.prev_left && !sample.left {
            if self.mode == CanvasMode::Link {
                if let Some(from) = self.link_from {
                    out.linked = Some(Link {
                        from,
                        to: self.card_at(sample.cursor),
                    });
                }
            }
            self.link_from = None;
        }
        self.prev_left = sample.left;
        out
    }

    /// **Draw the canvas** into `out` at `layer`: ground, edges, cards, then the
    /// in-flight rubber band over them all.
    ///
    /// `nodes` and `edges` are this frame's content in the same order the matching
    /// [`GraphCanvas::layout`] was given; the geometry is the one built there, so
    /// what lights up is exactly what the pointer would hit.
    pub fn draw(
        &self,
        nodes: &[GraphNode],
        edges: &[GraphEdge],
        style: &CanvasStyle,
        layer: f32,
        out: &mut Vec<HudCommand>,
    ) {
        let m = &self.metrics;
        let z = self.view.zoom;
        out.push(panel(
            self.area, style.bg, style.bg, 0.0, 0.0, 0.0, style.bg, layer,
        ));

        // Edges first so the cards sit on top of them.
        for g in &self.edges {
            let ink = edges.get(g.id).map(|e| e.ink).unwrap_or_default();
            let (color, width) = match ink {
                EdgeInk::Idle => (style.edge, m.edge_width),
                EdgeInk::Lit => (style.edge_lit, m.edge_width_lit),
                EdgeInk::Selected => (style.edge_lit, m.edge_width_selected),
            };
            for (a, b) in &g.segs[..g.n] {
                out.push(line(*a, *b, width, color, layer));
            }
        }

        for (i, n) in nodes.iter().enumerate() {
            let Some(&c) = self.cards.get(i) else {
                continue;
            };
            out.push(panel(
                c,
                style.card_fill_top,
                style.card_fill_bot,
                1.0,
                m.radius * z,
                if n.selected {
                    m.border_selected
                } else {
                    m.border
                },
                if n.selected {
                    style.card_border_selected
                } else {
                    style.card_border
                },
                layer,
            ));
            if n.icon {
                if let Some(icon) = self.icon_rect(i) {
                    out.push(panel(
                        icon,
                        style.icon_top,
                        style.icon_bot,
                        1.0,
                        (m.radius - 1.0).max(0.0) * z,
                        m.border,
                        style.icon_border,
                        layer,
                    ));
                }
            }
            let text_x = if n.icon {
                self.text_x()
            } else {
                m.card_pad * z
            };
            out.push(text(
                n.title,
                c.pos + Vec2::new(text_x, m.title_y * z),
                m.title_size * z,
                if n.selected {
                    style.label_selected
                } else {
                    style.label
                },
                layer,
            ));
            for (k, meta) in n.meta.iter().enumerate() {
                out.push(text(
                    meta,
                    c.pos + Vec2::new(text_x, (m.meta_y + k as f32 * m.meta_step) * z),
                    m.meta_size * z,
                    style.meta,
                    layer,
                ));
            }
            for k in 0..n.ports as usize {
                if let Some(p) = self.port_center(i, k, nodes) {
                    let d = m.port * z;
                    out.push(panel(
                        Rect {
                            pos: p - Vec2::splat(d * 0.5),
                            size: Vec2::splat(d),
                        },
                        style.port,
                        style.port,
                        0.0,
                        d * 0.5,
                        0.0,
                        style.port,
                        layer,
                    ));
                }
            }
        }

        // Rubber-band for an in-flight link drag, last so it rides over the cards.
        if let Some(from) = self.link_from.and_then(|i| self.cards.get(i)) {
            out.push(line(
                center(*from),
                self.cursor,
                m.link_width,
                style.link,
                layer,
            ));
        }
    }
}

/// The `i`th default grid slot, in canvas-local coordinates.
fn grid_slot(i: usize, cols: usize, m: &CanvasMetrics) -> Vec2 {
    let (c, r) = (i % cols.max(1), i / cols.max(1));
    Vec2::new(
        m.pad + c as f32 * (m.card.x + m.gap.x),
        m.pad + r as f32 * (m.card.y + m.gap.y),
    )
}

pub(crate) fn center(r: Rect) -> Vec2 {
    r.pos + r.size * 0.5
}

/// Where an edge meets two cards: the segment between their centres, clipped to each
/// card's border so it starts and ends ON the edge rather than under the card.
pub fn edge_points(from: Rect, to: Rect) -> (Vec2, Vec2) {
    let (a, b) = (center(from), center(to));
    (clip_to_border(from, a, b), clip_to_border(to, b, a))
}

/// Walk from `centre` toward `target` until leaving `rect` — the border crossing.
fn clip_to_border(rect: Rect, centre: Vec2, target: Vec2) -> Vec2 {
    let d = target - centre;
    let (hw, hh) = (rect.size.x * 0.5, rect.size.y * 0.5);
    if d.x.abs() < f32::EPSILON && d.y.abs() < f32::EPSILON {
        return centre;
    }
    // Scale the direction so it just touches the box: the smaller axis ratio wins.
    let tx = if d.x.abs() > f32::EPSILON {
        hw / d.x.abs()
    } else {
        f32::INFINITY
    };
    let ty = if d.y.abs() > f32::EPSILON {
        hh / d.y.abs()
    } else {
        f32::INFINITY
    };
    centre + d * tx.min(ty)
}

/// Distance from `p` to the segment `a`–`b`.
pub fn dist_to_segment(p: Vec2, a: Vec2, b: Vec2) -> f32 {
    let ab = b - a;
    let len2 = ab.length_squared();
    if len2 < 1e-6 {
        return (p - a).length();
    }
    let t = ((p - a).dot(ab) / len2).clamp(0.0, 1.0);
    (p - (a + ab * t)).length()
}

// ── HudCommand shorthands ─────────────────────────────────────────────────────
// One place each, so a filler's draw reads as geometry rather than as struct
// literals, and so both fillers emit byte-identical command shapes.

#[allow(clippy::too_many_arguments)] // each is a distinct panel attribute
pub(crate) fn panel(
    r: Rect,
    color: [f32; 4],
    color2: [f32; 4],
    grad: f32,
    radius: f32,
    border: f32,
    border_color: [f32; 4],
    layer: f32,
) -> HudCommand {
    HudCommand::Panel {
        x: r.pos.x,
        y: r.pos.y,
        w: r.size.x,
        h: r.size.y,
        color,
        color2,
        grad,
        radius,
        border,
        border_color,
        feather: 0.0,
        layer,
    }
}

pub(crate) fn line(a: Vec2, b: Vec2, width: f32, color: [f32; 4], layer: f32) -> HudCommand {
    HudCommand::Line {
        from: [a.x, a.y],
        to: [b.x, b.y],
        width,
        color,
        layer,
    }
}

pub(crate) fn text(s: &str, at: Vec2, size: f32, color: [f32; 4], layer: f32) -> HudCommand {
    HudCommand::Text {
        x: at.x,
        y: at.y,
        text: s.to_string(),
        size,
        color,
        layer,
        align: TextAlign::Left,
        font: FontRole::Body,
        italic: false,
        bold: false,
        // Negative = the role's own letter-spacing, matching `Renderer::draw_text`.
        tracking: -1.0,
        wrap: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn area() -> Rect {
        Rect {
            pos: Vec2::new(56.0, 96.0),
            size: Vec2::new(1304.0, 794.0),
        }
    }

    fn names(n: usize) -> Vec<String> {
        (0..n).map(|i| format!("State {i}")).collect()
    }

    fn keys(v: &[String]) -> Vec<&str> {
        v.iter().map(String::as_str).collect()
    }

    /// A canvas seated on `area()` with `n` default-grid cards and no edges.
    fn seated(n: usize) -> (GraphCanvas, Vec<String>) {
        let mut c = GraphCanvas::new(CanvasMetrics::default());
        c.seat(area());
        let ns = names(n);
        c.layout(&keys(&ns), &[]);
        (c, ns)
    }

    fn sample(cursor: Vec2) -> PointerSample {
        PointerSample {
            cursor,
            inside: true,
            ..PointerSample::default()
        }
    }

    #[test]
    fn grid_layout_wraps_into_rows_inside_the_area() {
        let (c, _) = seated(9);
        let cards = c.cards();
        assert_eq!(cards.len(), 9);
        for r in cards {
            assert!(
                r.pos.x >= area().pos.x && r.pos.y >= area().pos.y,
                "card escapes the area"
            );
        }
        // Row 0 shares a y; the first card of row 1 sits lower and back at the left.
        let cols = cards.iter().filter(|r| r.pos.y == cards[0].pos.y).count();
        assert!(cols >= 2, "expected several columns at this width");
        assert!(cards[cols].pos.y > cards[0].pos.y, "wraps to a new row");
        assert_eq!(
            cards[cols].pos.x, cards[0].pos.x,
            "new row restarts at the left"
        );
    }

    #[test]
    fn grid_layout_handles_empty_and_narrow() {
        let mut c = GraphCanvas::new(CanvasMetrics::default());
        c.seat(area());
        c.layout(&[], &[]);
        assert!(c.cards().is_empty());

        c.seat(Rect {
            pos: Vec2::ZERO,
            size: Vec2::new(10.0, 400.0),
        });
        let ns = names(3);
        c.layout(&keys(&ns), &[]);
        // One column minimum — never a divide-by-zero or an empty layout.
        assert_eq!(c.cards().len(), 3);
        assert_eq!(c.cards()[0].pos.x, c.cards()[1].pos.x);
        assert!(c.cards()[1].pos.y > c.cards()[0].pos.y);
    }

    #[test]
    fn card_at_finds_the_card_under_the_cursor() {
        let (c, _) = seated(4);
        let card = c.cards()[2];
        assert_eq!(c.card_at(center(card)), Some(2));
        assert_eq!(
            c.card_at(card.pos - Vec2::new(5.0, 5.0)),
            None,
            "gap is empty"
        );
        assert_eq!(c.card_at(Vec2::new(-100.0, -100.0)), None);
    }

    /// A hand-placed card ignores its grid slot; the rest keep theirs. Placement is
    /// by KEY, so inserting a node ahead of it must not drag it somewhere else.
    #[test]
    fn a_placed_card_stays_put_when_the_graph_changes_around_it() {
        let (mut c, ns) = seated(4);
        let before: Vec<Rect> = c.cards().to_vec();
        c.view
            .placed
            .insert("State 2".into(), Vec2::new(500.0, 400.0));
        c.layout(&keys(&ns), &[]);
        assert_eq!(
            c.cards()[2].pos,
            c.view.to_screen(area(), Vec2::new(500.0, 400.0))
        );
        let placed = c.cards()[2].pos;

        // Insert a node at the front: "State 2" moves to index 3 but must not move
        // on screen; an unplaced neighbour DOES take the next grid slot.
        let mut shifted = names(4);
        shifted.insert(0, "State new".into());
        c.layout(&keys(&shifted), &[]);
        assert_eq!(
            c.cards()[3].pos,
            placed,
            "a placed card is pinned by key, not index"
        );
        assert_eq!(c.cards()[0].pos, before[0].pos, "grid slots are positional");
    }

    /// Pan and zoom round-trip: a screen point maps to a local point and back.
    #[test]
    fn view_transforms_round_trip() {
        for (pan, zoom) in [
            (Vec2::ZERO, 1.0),
            (Vec2::new(-320.0, 90.0), 1.0),
            (Vec2::new(15.0, -60.0), 0.5),
            (Vec2::new(-4.0, 7.0), 2.0),
        ] {
            let v = View {
                pan,
                zoom,
                ..View::new()
            };
            let p = Vec2::new(742.0, 415.0);
            let back = v.to_screen(area(), v.to_local(area(), p));
            assert!(
                (back - p).length() < 1e-3,
                "pan {pan} zoom {zoom} lost the point"
            );
        }
    }

    /// Wheel-zoom is anchored: the graph point under the cursor stays under the
    /// cursor, so zooming reads as moving a camera rather than teleporting the graph.
    #[test]
    fn wheel_zoom_keeps_the_cursor_over_the_same_point() {
        let (mut c, ns) = seated(4);
        let cursor = Vec2::new(900.0, 500.0);
        let before = c.view.to_local(area(), cursor);
        c.pointer(
            &PointerSample {
                wheel: 2.0,
                ..sample(cursor)
            },
            &keys(&ns),
        );
        let after = c.view.to_local(area(), cursor);
        assert!((after - before).length() < 1e-3, "the anchor drifted");
        assert!(c.zoom() > 1.0, "zoom actually changed");

        // Outside the seat the wheel is not the canvas's business.
        let z = c.zoom();
        c.pointer(
            &PointerSample {
                wheel: 2.0,
                inside: false,
                ..sample(cursor)
            },
            &keys(&ns),
        );
        assert_eq!(c.zoom(), z, "a wheel outside the canvas must not zoom it");
    }

    /// Zoom is clamped, and stays clamped no matter how far the wheel is spun.
    #[test]
    fn zoom_clamps_at_both_ends() {
        let m = CanvasMetrics::default();
        let mut v = View::new();
        for _ in 0..64 {
            v.zoom_at(area(), Vec2::new(700.0, 400.0), 1.3, m.zoom_min, m.zoom_max);
        }
        assert!(
            (v.zoom - m.zoom_max).abs() < 1e-4,
            "zoom ran past the ceiling: {}",
            v.zoom
        );
        for _ in 0..128 {
            v.zoom_at(area(), Vec2::new(700.0, 400.0), 0.8, m.zoom_min, m.zoom_max);
        }
        assert!(
            (v.zoom - m.zoom_min).abs() < 1e-4,
            "zoom ran past the floor: {}",
            v.zoom
        );
        assert!(v.pan.is_finite());
    }

    /// Cards scale with the zoom, and picking follows them — otherwise a zoomed-out
    /// graph would be unclickable where it is drawn.
    #[test]
    fn picking_follows_zoom_and_pan() {
        let mut c = GraphCanvas::new(CanvasMetrics::default());
        c.seat(area());
        c.view.pan = Vec2::new(-120.0, 40.0);
        c.view.zoom = 0.5;
        let ns = names(4);
        c.layout(&keys(&ns), &[]);
        assert!(
            (c.cards()[0].size.x - CanvasMetrics::default().card.x * 0.5).abs() < 1e-3,
            "cards scale with zoom"
        );
        for i in 0..4 {
            assert_eq!(c.card_at(center(c.cards()[i])), Some(i));
        }
        // And a drop lands where the pointer is: placing a card at a screen point and
        // laying out again puts it back under that point.
        let target = Vec2::new(800.0, 300.0);
        c.view
            .placed
            .insert("State 1".into(), c.view.to_local(area(), target));
        c.layout(&keys(&ns), &[]);
        assert!((c.cards()[1].pos - target).length() < 1e-3);
    }

    #[test]
    fn edge_points_start_on_the_borders_not_the_centres() {
        let a = Rect {
            pos: Vec2::new(0.0, 0.0),
            size: Vec2::new(200.0, 100.0),
        };
        let b = Rect {
            pos: Vec2::new(400.0, 0.0),
            size: Vec2::new(200.0, 100.0),
        };
        let (p, q) = edge_points(a, b);
        assert!((p.x - 200.0).abs() < 0.01, "left card's right border");
        assert!((q.x - 400.0).abs() < 0.01, "right card's left border");
        assert!(
            (p.y - 50.0).abs() < 0.01 && (q.y - 50.0).abs() < 0.01,
            "mid-height"
        );
        assert!(p.x > center(a).x && q.x < center(b).x);
    }

    #[test]
    fn edge_points_are_stable_for_coincident_cards() {
        let a = Rect {
            pos: Vec2::ZERO,
            size: Vec2::new(100.0, 50.0),
        };
        let (p, q) = edge_points(a, a);
        assert_eq!(p, center(a));
        assert_eq!(q, center(a));
    }

    #[test]
    fn distance_to_a_degenerate_segment_is_the_point_distance() {
        let p = Vec2::new(3.0, 4.0);
        assert!((dist_to_segment(p, Vec2::ZERO, Vec2::ZERO) - 5.0).abs() < 1e-4);
        // Endpoints, not the infinite line: a point beyond the end measures to the end.
        let d = dist_to_segment(Vec2::new(20.0, 0.0), Vec2::ZERO, Vec2::new(10.0, 0.0));
        assert!((d - 10.0).abs() < 1e-4, "clamped to the segment, got {d}");
    }

    /// Edges are 1–3px lines, so picking one needs a grab radius far wider than what
    /// is drawn — and where several run close together the NEAREST must win rather
    /// than the first, or overlapping edges would be unselectable.
    #[test]
    fn edges_pick_by_proximity_within_the_grab_radius() {
        // Two cards stacked so their edge runs vertically between the centres, plus a
        // third pair beside it — picking is exercised through the real layout.
        let mut c = GraphCanvas::new(CanvasMetrics::default());
        c.seat(area());
        let ns = names(2);
        let edges = [GraphEdge {
            from: 0,
            to: 1,
            ..GraphEdge::default()
        }];
        c.layout(&keys(&ns), &edges);
        let (a, b) = edge_points(c.cards()[0], c.cards()[1]);
        let mid = (a + b) * 0.5;
        assert_eq!(c.edge_at(mid), Some(0), "dead on the edge");
        let n = (b - a).normalize();
        let perp = Vec2::new(-n.y, n.x);
        let grab = CanvasMetrics::default().edge_grab;
        assert_eq!(c.edge_at(mid + perp * (grab - 0.5)), Some(0), "within grab");
        assert_eq!(c.edge_at(mid + perp * (grab + 0.5)), None, "beyond grab");
        assert_eq!(c.edge_at(a - n * 40.0), None, "past the start");
    }

    /// A self-edge is a three-segment arc over its card — a straight line to itself
    /// would vanish inside the card and be unpickable.
    #[test]
    fn a_self_edge_arcs_over_its_card_and_can_be_picked() {
        let mut c = GraphCanvas::new(CanvasMetrics::default());
        c.seat(area());
        let ns = names(1);
        c.layout(
            &keys(&ns),
            &[GraphEdge {
                from: 0,
                to: 0,
                ..GraphEdge::default()
            }],
        );
        assert_eq!(c.edges.len(), 1);
        assert_eq!(c.edges[0].n, 3, "three segments");
        let card = c.cards()[0];
        let lift = CanvasMetrics::default().self_loop_lift;
        // The arc rises ABOVE the card, so it is visible and hittable outside it.
        for (a, b) in &c.edges[0].segs[..3] {
            assert!(a.y <= card.pos.y + 0.01 && b.y <= card.pos.y + 0.01);
        }
        let top = Vec2::new(center(card).x, card.pos.y - lift);
        assert_eq!(c.edge_at(top), Some(0), "the top bar is what is picked");
        // And a card sitting over it still wins the press (cards draw on top).
        assert_eq!(c.card_at(center(card)), Some(0));
    }

    /// An edge naming a node that is not laid out is skipped WITHOUT shifting the
    /// identities of the ones that are — otherwise a pick would select a different
    /// transition than the one drawn.
    #[test]
    fn a_dangling_edge_is_skipped_without_renumbering_the_rest() {
        let mut c = GraphCanvas::new(CanvasMetrics::default());
        c.seat(area());
        let ns = names(2);
        let edges = [
            GraphEdge {
                from: 0,
                to: 9,
                ..GraphEdge::default()
            },
            GraphEdge {
                from: 0,
                to: 1,
                ..GraphEdge::default()
            },
        ];
        c.layout(&keys(&ns), &edges);
        assert_eq!(c.edges.len(), 1);
        let (a, b) = edge_points(c.cards()[0], c.cards()[1]);
        assert_eq!(
            c.edge_at((a + b) * 0.5),
            Some(1),
            "the surviving edge keeps its own index"
        );
    }

    /// Select mode: a press picks the card up and the drag writes its placement
    /// under the node's KEY — never into the consumer's document.
    #[test]
    fn select_mode_drags_a_card_by_its_key() {
        let (mut c, ns) = seated(4);
        c.set_mode(CanvasMode::Select);
        let start = center(c.cards()[1]);
        let ev = c.pointer(
            &PointerSample {
                pressed: true,
                left: true,
                ..sample(start)
            },
            &keys(&ns),
        );
        assert_eq!(ev.pressed.unwrap().card, Some(1));
        assert!(ev.linked.is_none());

        let to = start + Vec2::new(120.0, -40.0);
        c.pointer(
            &PointerSample {
                left: true,
                ..sample(to)
            },
            &keys(&ns),
        );
        assert!(c.view.placed.contains_key("State 1"), "placed by key");
        c.layout(&keys(&ns), &[]);
        assert!(
            (center(c.cards()[1]) - to).length() < 1e-3,
            "the card followed the cursor from where it was grabbed"
        );

        // Releasing ends the drag: later motion must not keep moving the card.
        c.pointer(&sample(to), &keys(&ns));
        let parked = c.view.placed["State 1"];
        c.pointer(&sample(to + Vec2::new(300.0, 300.0)), &keys(&ns));
        assert_eq!(c.view.placed["State 1"], parked, "the drag ended");
    }

    /// Inspect mode reports what a press hit and moves nothing — the shape a
    /// create/delete tool needs.
    #[test]
    fn inspect_mode_reports_hits_without_dragging() {
        let (mut c, ns) = seated(3);
        c.set_mode(CanvasMode::Inspect);
        let at = center(c.cards()[2]);
        let ev = c.pointer(
            &PointerSample {
                pressed: true,
                left: true,
                ..sample(at)
            },
            &keys(&ns),
        );
        assert_eq!(ev.pressed.unwrap().card, Some(2));
        c.pointer(
            &PointerSample {
                left: true,
                ..sample(at + Vec2::new(200.0, 0.0))
            },
            &keys(&ns),
        );
        assert!(c.view.placed.is_empty(), "inspect never moves a card");
    }

    /// A press on empty canvas reports the LOCAL point, so a consumer that creates a
    /// node there does not have to invert the view itself.
    #[test]
    fn a_press_on_empty_canvas_reports_where_it_landed() {
        let (mut c, ns) = seated(2);
        let gap = c.cards()[0].pos - Vec2::new(12.0, 12.0);
        let ev = c
            .pointer(
                &PointerSample {
                    pressed: true,
                    left: true,
                    ..sample(gap)
                },
                &keys(&ns),
            )
            .pressed
            .unwrap();
        assert_eq!(ev.card, None);
        assert_eq!(ev.edge, None);
        assert!((c.view.to_screen(area(), ev.local) - gap).length() < 1e-3);
    }

    /// Link mode: press a card, release on another. The release edge is derived
    /// INSIDE the canvas, and a release on empty space still ends the drag.
    #[test]
    fn link_mode_reports_the_pair_and_always_ends_the_drag() {
        let (mut c, ns) = seated(4);
        c.set_mode(CanvasMode::Link);
        let from = center(c.cards()[0]);
        let to = center(c.cards()[3]);
        c.pointer(
            &PointerSample {
                pressed: true,
                left: true,
                ..sample(from)
            },
            &keys(&ns),
        );
        assert_eq!(c.linking(), Some(0), "the rubber band is in flight");
        let ev = c.pointer(&sample(to), &keys(&ns));
        assert_eq!(
            ev.linked,
            Some(Link {
                from: 0,
                to: Some(3)
            })
        );
        assert_eq!(c.linking(), None);

        // A self-link is REPORTED, not refused — whether it is legal is the
        // consumer's rule, not this filler's.
        c.pointer(
            &PointerSample {
                pressed: true,
                left: true,
                ..sample(from)
            },
            &keys(&ns),
        );
        assert_eq!(
            c.pointer(&sample(from), &keys(&ns)).linked,
            Some(Link {
                from: 0,
                to: Some(0)
            })
        );

        // A release over nothing ends the drag with no target.
        c.pointer(
            &PointerSample {
                pressed: true,
                left: true,
                ..sample(from)
            },
            &keys(&ns),
        );
        let out = Vec2::new(-500.0, -500.0);
        assert_eq!(
            c.pointer(&sample(out), &keys(&ns)).linked,
            Some(Link { from: 0, to: None })
        );
        assert_eq!(c.linking(), None, "no rubber band survives the release");
    }

    /// Changing tool mid-gesture abandons whatever was in flight rather than leaving
    /// a stranded drag that fires on the next unrelated release. So does `cancel`,
    /// which is what a consumer calls when its page stops driving the canvas —
    /// without it the release that landed off-page would weave a link on return.
    #[test]
    fn changing_mode_or_cancelling_abandons_the_gesture_in_flight() {
        for abandon in [
            (|c: &mut GraphCanvas| c.set_mode(CanvasMode::Select)) as fn(&mut GraphCanvas),
            |c: &mut GraphCanvas| c.cancel(),
        ] {
            let (mut c, ns) = seated(3);
            c.set_mode(CanvasMode::Link);
            c.pointer(
                &PointerSample {
                    pressed: true,
                    left: true,
                    ..sample(center(c.cards()[0]))
                },
                &keys(&ns),
            );
            assert_eq!(c.linking(), Some(0));
            abandon(&mut c);
            assert_eq!(c.linking(), None);
            assert!(c
                .pointer(&sample(center(c.cards()[1])), &keys(&ns))
                .linked
                .is_none());
        }
    }

    #[test]
    fn middle_drag_pans_and_stops_when_released() {
        let (mut c, ns) = seated(2);
        let a = Vec2::new(700.0, 400.0);
        c.pointer(
            &PointerSample {
                middle: true,
                ..sample(a)
            },
            &keys(&ns),
        );
        c.pointer(
            &PointerSample {
                middle: true,
                ..sample(a + Vec2::new(60.0, -20.0))
            },
            &keys(&ns),
        );
        assert_eq!(c.view.pan, Vec2::new(60.0, -20.0));
        // Released, then moved: the pan must not follow.
        c.pointer(&sample(a + Vec2::new(400.0, 400.0)), &keys(&ns));
        assert_eq!(c.view.pan, Vec2::new(60.0, -20.0));
    }

    /// Ports sit down a card's right edge and pick by proximity. A node that
    /// declares none has nothing to hit — its whole card is the connector.
    #[test]
    fn ports_sit_on_the_right_edge_and_pick_nearest() {
        let (c, _) = seated(2);
        let nodes = [
            GraphNode {
                ports: 2,
                ..GraphNode::default()
            },
            GraphNode::default(),
        ];
        let p0 = c.port_center(0, 0, &nodes).unwrap();
        let p1 = c.port_center(0, 1, &nodes).unwrap();
        let card = c.cards()[0];
        assert!(
            (p0.x - (card.pos.x + card.size.x)).abs() < 1e-3,
            "right edge"
        );
        assert!(p1.y > p0.y, "ports descend");
        assert!(
            p0.y > card.pos.y && p1.y < card.pos.y + card.size.y,
            "inside"
        );
        assert_eq!(c.port_at(p0, &nodes), Some((0, 0)));
        assert_eq!(c.port_at(p1 + Vec2::new(1.0, 1.0), &nodes), Some((0, 1)));
        assert_eq!(
            c.port_at(p0 + Vec2::new(0.0, 60.0), &nodes),
            None,
            "too far"
        );
        assert_eq!(c.port_center(1, 0, &nodes), None, "no ports declared");
        assert_eq!(c.port_at(center(c.cards()[1]), &nodes), None);
    }

    /// The draw emits real primitives — edges as `HudCommand::Line` (the line
    /// primitive, not a thin axis-aligned rect that could only be drawn horizontal
    /// or vertical) — and every colour comes from the style the consumer filled.
    #[test]
    fn the_draw_emits_lines_for_edges_and_uses_only_the_given_style() {
        let mut c = GraphCanvas::new(CanvasMetrics::default());
        c.seat(area());
        let ns = names(2);
        let edges = [GraphEdge {
            from: 0,
            to: 1,
            ink: EdgeInk::Selected,
        }];
        c.layout(&keys(&ns), &edges);
        let meta: [&str; 1] = ["IN 0 · OUT 1"];
        let nodes = [
            GraphNode {
                title: "Idle",
                meta: &meta,
                selected: true,
                icon: true,
                ports: 0,
            },
            GraphNode {
                title: "Run",
                ..GraphNode::default()
            },
        ];
        let style = CanvasStyle {
            bg: [0.1, 0.1, 0.1, 1.0],
            edge_lit: [1.0, 0.0, 0.0, 1.0],
            label: [0.0, 1.0, 0.0, 1.0],
            ..CanvasStyle::blank()
        };
        let mut out = Vec::new();
        c.draw(&nodes, &edges, &style, 3.0, &mut out);

        let lines: Vec<_> = out
            .iter()
            .filter_map(|cmd| match cmd {
                HudCommand::Line {
                    color,
                    width,
                    layer,
                    ..
                } => Some((*color, *width, *layer)),
                _ => None,
            })
            .collect();
        assert_eq!(lines.len(), 1, "one edge, one line — not a quad fake");
        assert_eq!(lines[0].0, style.edge_lit);
        assert_eq!(lines[0].1, CanvasMetrics::default().edge_width_selected);
        assert_eq!(lines[0].2, 3.0, "every command carries the caller's layer");

        let texts: Vec<&str> = out
            .iter()
            .filter_map(|cmd| match cmd {
                HudCommand::Text { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(texts, ["Idle", "IN 0 · OUT 1", "Run"]);
        // The icon backdrop is drawn only for the node that asked for one: ground +
        // 2 cards + 1 icon = 4 panels, and no port dots.
        assert_eq!(
            out.iter()
                .filter(|c| matches!(c, HudCommand::Panel { .. }))
                .count(),
            4
        );
        // Nothing painted with a colour the consumer did not supply.
        for cmd in &out {
            let c = match cmd {
                HudCommand::Panel { color, .. } => *color,
                HudCommand::Line { color, .. } => *color,
                HudCommand::Text { color, .. } => *color,
                _ => continue,
            };
            assert!(
                [
                    style.bg,
                    style.edge_lit,
                    style.label,
                    style.card_fill_top,
                    style.icon_top,
                    style.meta,
                    style.label_selected
                ]
                .contains(&c),
                "unexpected colour {c:?} — the crate must hold no rgba literals"
            );
        }
    }

    /// The in-flight rubber band is drawn from the source card to the cursor, over
    /// the cards — without it a link drag has no feedback at all.
    #[test]
    fn the_link_rubber_band_is_drawn_last() {
        let (mut c, ns) = seated(2);
        c.set_mode(CanvasMode::Link);
        let at = center(c.cards()[0]);
        c.pointer(
            &PointerSample {
                pressed: true,
                left: true,
                ..sample(at)
            },
            &keys(&ns),
        );
        let cursor = at + Vec2::new(240.0, 90.0);
        c.pointer(
            &PointerSample {
                left: true,
                ..sample(cursor)
            },
            &keys(&ns),
        );
        let nodes = [GraphNode::default(), GraphNode::default()];
        let style = CanvasStyle {
            link: [0.0, 0.0, 1.0, 1.0],
            ..CanvasStyle::blank()
        };
        let mut out = Vec::new();
        c.draw(&nodes, &[], &style, 0.0, &mut out);
        let last = out.last().unwrap();
        match last {
            HudCommand::Line {
                from, to, color, ..
            } => {
                assert_eq!(*color, style.link);
                assert!((Vec2::new(from[0], from[1]) - center(c.cards()[0])).length() < 1e-3);
                assert_eq!(*to, [cursor.x, cursor.y]);
            }
            other => panic!("expected the rubber band last, got {other:?}"),
        }
    }

    /// A fresh document is a fresh workspace: the camera and every hand placement go
    /// with it, and no gesture survives into the new graph.
    #[test]
    fn reset_view_clears_the_camera_the_placements_and_any_gesture() {
        let (mut c, ns) = seated(3);
        c.set_mode(CanvasMode::Link);
        c.view.pan = Vec2::new(50.0, 50.0);
        c.view.placed.insert("State 0".into(), Vec2::ZERO);
        c.pointer(
            &PointerSample {
                pressed: true,
                left: true,
                ..sample(center(c.cards()[0]))
            },
            &keys(&ns),
        );
        c.reset_view();
        assert_eq!(c.view.pan, Vec2::ZERO);
        assert_eq!(c.zoom(), 1.0);
        assert!(c.view.placed.is_empty());
        assert_eq!(c.linking(), None);
    }

    /// An unseated canvas (an off-screen surface reserves a zero rect) picks nothing
    /// rather than matching every point at the origin.
    #[test]
    fn an_unseated_canvas_still_lays_out_but_picks_nothing_off_it() {
        let mut c = GraphCanvas::new(CanvasMetrics::default());
        let ns = names(2);
        c.layout(&keys(&ns), &[]);
        assert_eq!(c.cards().len(), 2);
        assert_eq!(c.card_at(Vec2::new(-10.0, -10.0)), None);
        assert_eq!(c.edge_at(Vec2::new(-10.0, -10.0)), None);
    }
}
