//! The **scene-owned** node-graph canvas for the State Machine page.
//!
//! This is drawn by the scene, not by the `flicker-widgets` walker: it needs free 2D
//! positioning, transition edges (curves/arrows the HUD has no command for), and drop
//! targets that the walker's closed component set has no templates for. The walker
//! still owns the chrome around it (rails, buttons, lists).
//!
//! Layout, hit-testing, and edge geometry are **pure** (no GPU, no renderer), so they
//! are unit-tested directly; only [`draw`] touches the renderer.

use flicker::render::Vec2;

/// A card's box on screen.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CardRect {
    pub pos: Vec2,
    pub size: Vec2,
}

impl CardRect {
    pub fn contains(&self, p: Vec2) -> bool {
        p.x >= self.pos.x
            && p.x <= self.pos.x + self.size.x
            && p.y >= self.pos.y
            && p.y <= self.pos.y + self.size.y
    }
    pub fn center(&self) -> Vec2 {
        self.pos + self.size * 0.5
    }
}

/// The rectangle the canvas occupies (between the tool rail, the pack rail, and the
/// TAE strip).
#[derive(Clone, Copy, Debug)]
pub struct CanvasArea {
    pub pos: Vec2,
    pub size: Vec2,
}

impl CanvasArea {
    /// Whether the pointer is over the graph. Tools act only inside this rect, so a click
    /// on the timeline or on empty chrome can never place or delete a state.
    pub fn contains(&self, p: Vec2) -> bool {
        p.x >= self.pos.x
            && p.x <= self.pos.x + self.size.x
            && p.y >= self.pos.y
            && p.y <= self.pos.y + self.size.y
    }
}

/// Card geometry — matches the design's 220px-wide state card.
pub const CARD_W: f32 = 220.0;
pub const CARD_H: f32 = 88.0;
/// Margin between a card's border and the doll inside it.
pub const CARD_PAD: f32 = 10.0;
/// The live doll on a state card, at zoom 1.
pub const CARD_STAGE: f32 = 68.0;
const GAP_X: f32 = 80.0;
const GAP_Y: f32 = 42.0;
const PAD: f32 = 28.0;
/// Zoom limits — far enough out to see a 23-state graph whole, far enough in to read a
/// card's clip name.
pub const ZOOM_MIN: f32 = 0.35;
pub const ZOOM_MAX: f32 = 2.0;

/// How the author has arranged their workspace: where the graph is panned and scaled to,
/// and any cards they have placed by hand.
///
/// **None of this is part of the state machine.** It is never written into the pack — the
/// runtime has no concept of where a state sits on a screen. Hand positions are keyed by
/// state NAME rather than index, so they survive states being added, removed, or
/// reordered underneath them.
#[derive(Clone, Debug)]
pub struct View {
    pub pan: Vec2,
    pub zoom: f32,
    pub placed: std::collections::HashMap<String, Vec2>,
}

impl Default for View {
    fn default() -> Self {
        Self {
            pan: Vec2::ZERO,
            zoom: 1.0,
            placed: std::collections::HashMap::new(),
        }
    }
}

impl View {
    /// Canvas-local point → screen pixels.
    pub fn to_screen(&self, area: CanvasArea, local: Vec2) -> Vec2 {
        area.pos + local * self.zoom + self.pan
    }

    /// Screen pixels → canvas-local point. The inverse of [`View::to_screen`].
    pub fn to_local(&self, area: CanvasArea, screen: Vec2) -> Vec2 {
        (screen - area.pos - self.pan) / self.zoom
    }

    /// Zoom about a fixed screen point — the cursor stays over the same part of the graph,
    /// which is what makes wheel-zoom feel anchored instead of lurching.
    pub fn zoom_at(&mut self, area: CanvasArea, anchor: Vec2, factor: f32) {
        let before = self.to_local(area, anchor);
        self.zoom = (self.zoom * factor).clamp(ZOOM_MIN, ZOOM_MAX);
        let after = self.to_local(area, anchor);
        // Re-pan so `anchor` maps back to the same local point it did before.
        self.pan += (after - before) * self.zoom;
    }
}

/// Place `names`' cards: each one where the author dropped it, else on the default grid.
///
/// The design pins every node's `x`/`y` by hand, but a real pack has 14–23 states, so the
/// editor seeds a grid and lets the author rearrange from there.
pub fn layout<'a>(
    names: impl Iterator<Item = &'a str>,
    area: CanvasArea,
    view: &View,
) -> Vec<CardRect> {
    let cols = columns(area);
    let size = Vec2::new(CARD_W, CARD_H) * view.zoom;
    names
        .enumerate()
        .map(|(i, name)| CardRect {
            pos: view.to_screen(
                area,
                view.placed.get(name).copied().unwrap_or(grid_slot(i, cols)),
            ),
            size,
        })
        .collect()
}

/// Columns in the default grid. Derived from the UNZOOMED area, so zooming moves the
/// camera over a fixed arrangement instead of reflowing it under the author.
fn columns(area: CanvasArea) -> usize {
    let usable = (area.size.x - PAD * 2.0).max(CARD_W);
    (((usable + GAP_X) / (CARD_W + GAP_X)).floor() as usize).max(1)
}

/// The `i`th default grid slot, in canvas-local coordinates.
fn grid_slot(i: usize, cols: usize) -> Vec2 {
    let (c, r) = (i % cols.max(1), i / cols.max(1));
    Vec2::new(
        PAD + c as f32 * (CARD_W + GAP_X),
        PAD + r as f32 * (CARD_H + GAP_Y),
    )
}

/// The doll's rect inside a card. The scene draws the backdrop from this and the stage
/// request reserves the image from it, so the two cannot drift apart — and both scale
/// with the zoom, like everything else on a card.
pub fn card_stage_rect(card: CardRect, zoom: f32) -> CardRect {
    CardRect {
        pos: card.pos + Vec2::splat(CARD_PAD * zoom),
        size: Vec2::splat(CARD_STAGE * zoom),
    }
}

/// Where a card's text column starts, measured from the card's left edge.
pub fn card_text_x(zoom: f32) -> f32 {
    (CARD_PAD * 2.0 + CARD_STAGE) * zoom
}

/// Topmost card under `p` (last wins, matching draw order).
pub fn hit_test(cards: &[CardRect], p: Vec2) -> Option<usize> {
    cards
        .iter()
        .enumerate()
        .rev()
        .find(|(_, c)| c.contains(p))
        .map(|(i, _)| i)
}

/// Where a transition edge meets the two cards: the segment between their centres,
/// clipped to each card's border so the arrow starts and ends ON the edge rather than
/// under the card.
pub fn edge_points(from: CardRect, to: CardRect) -> (Vec2, Vec2) {
    let (a, b) = (from.center(), to.center());
    (clip_to_border(from, a, b), clip_to_border(to, b, a))
}

/// How near an edge the pointer must be to grab it (screen pixels). Edges are 1–2px
/// quads, so picking needs a tolerance far wider than what is drawn.
pub const EDGE_GRAB: f32 = 7.0;

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

/// Pick the edge nearest the pointer, within [`EDGE_GRAB`].
///
/// `edges` is `(id, from_point, to_point)`; the id is whatever the caller uses to name a
/// transition, so this stays pure geometry and knows nothing about the pack. Nearest
/// wins rather than first, so overlapping edges pick the one actually under the cursor.
pub fn hit_edge<T: Copy>(edges: &[(T, Vec2, Vec2)], p: Vec2) -> Option<T> {
    edges
        .iter()
        .map(|(id, a, b)| (*id, dist_to_segment(p, *a, *b)))
        .filter(|(_, d)| *d <= EDGE_GRAB)
        .min_by(|x, y| x.1.total_cmp(&y.1))
        .map(|(id, _)| id)
}

/// Walk from `centre` toward `target` until leaving `rect` — the border crossing.
fn clip_to_border(rect: CardRect, centre: Vec2, target: Vec2) -> Vec2 {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn area() -> CanvasArea {
        CanvasArea {
            pos: Vec2::new(56.0, 96.0),
            size: Vec2::new(1304.0, 794.0),
        }
    }

    fn names(n: usize) -> Vec<String> {
        (0..n).map(|i| format!("State {i}")).collect()
    }

    #[test]
    fn grid_layout_wraps_into_rows_inside_the_area() {
        let v = View::default();
        let cards = layout(names(9).iter().map(String::as_str), area(), &v);
        assert_eq!(cards.len(), 9);
        // Every card starts inside the area.
        for c in &cards {
            assert!(
                c.pos.x >= area().pos.x && c.pos.y >= area().pos.y,
                "card escapes the area"
            );
        }
        // Row 0 shares a y; the first card of row 1 sits lower and back at the left.
        let cols = cards.iter().filter(|c| c.pos.y == cards[0].pos.y).count();
        assert!(cols >= 2, "expected several columns at this width");
        assert!(cards[cols].pos.y > cards[0].pos.y, "wraps to a new row");
        assert_eq!(
            cards[cols].pos.x, cards[0].pos.x,
            "new row restarts at the left"
        );
    }

    #[test]
    fn grid_layout_handles_empty_and_narrow() {
        let v = View::default();
        assert!(layout([].into_iter(), area(), &v).is_empty());
        let narrow = CanvasArea {
            pos: Vec2::ZERO,
            size: Vec2::new(10.0, 400.0),
        };
        let cards = layout(names(3).iter().map(String::as_str), narrow, &v);
        // One column minimum — never a divide-by-zero or an empty layout.
        assert_eq!(cards.len(), 3);
        assert_eq!(cards[0].pos.x, cards[1].pos.x);
        assert!(cards[1].pos.y > cards[0].pos.y);
    }

    #[test]
    fn hit_test_finds_the_card_under_the_cursor() {
        let cards = layout(
            names(4).iter().map(String::as_str),
            area(),
            &View::default(),
        );
        let c = cards[2];
        assert_eq!(hit_test(&cards, c.center()), Some(2));
        assert_eq!(
            hit_test(&cards, c.pos - Vec2::new(5.0, 5.0)),
            None,
            "gap is empty"
        );
        assert_eq!(hit_test(&cards, Vec2::new(-100.0, -100.0)), None);
    }

    /// A hand-placed card ignores its grid slot; the rest keep theirs. Placement is by
    /// NAME, so inserting a state ahead of it must not drag it somewhere else.
    #[test]
    fn a_placed_card_stays_put_when_the_graph_changes_around_it() {
        let mut v = View::default();
        v.placed.insert("State 2".into(), Vec2::new(500.0, 400.0));
        let before = layout(names(4).iter().map(String::as_str), area(), &v);
        assert_eq!(before[2].pos, v.to_screen(area(), Vec2::new(500.0, 400.0)));

        // Insert a state at the front: "State 2" moves to index 3 but must not move on screen.
        let mut shifted = names(4);
        shifted.insert(0, "State new".into());
        let after = layout(shifted.iter().map(String::as_str), area(), &v);
        assert_eq!(
            after[3].pos, before[2].pos,
            "a placed card is pinned by name, not index"
        );
        // An unplaced neighbour DOES take the next grid slot, as it should.
        assert_eq!(after[0].pos, before[0].pos, "grid slots are positional");
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
                ..View::default()
            };
            let p = Vec2::new(742.0, 415.0);
            let back = v.to_screen(area(), v.to_local(area(), p));
            assert!(
                (back - p).length() < 1e-3,
                "pan {pan} zoom {zoom} lost the point"
            );
        }
    }

    /// Wheel-zoom is anchored: the graph point under the cursor stays under the cursor,
    /// so zooming reads as moving a camera rather than teleporting the layout.
    #[test]
    fn zoom_keeps_the_cursor_over_the_same_point() {
        let mut v = View::default();
        let cursor = Vec2::new(900.0, 500.0);
        let before = v.to_local(area(), cursor);
        v.zoom_at(area(), cursor, 1.25);
        let after = v.to_local(area(), cursor);
        assert!((after - before).length() < 1e-3, "the anchor drifted");
        assert!(v.zoom > 1.0, "zoom actually changed");
    }

    /// Zoom is clamped, and stays clamped no matter how far the wheel is spun.
    #[test]
    fn zoom_clamps_at_both_ends() {
        let mut v = View::default();
        for _ in 0..64 {
            v.zoom_at(area(), Vec2::new(700.0, 400.0), 1.3);
        }
        assert!(
            (v.zoom - ZOOM_MAX).abs() < 1e-4,
            "zoom ran past the ceiling: {}",
            v.zoom
        );
        for _ in 0..128 {
            v.zoom_at(area(), Vec2::new(700.0, 400.0), 0.8);
        }
        assert!(
            (v.zoom - ZOOM_MIN).abs() < 1e-4,
            "zoom ran past the floor: {}",
            v.zoom
        );
        // And the pan stayed finite through all of it.
        assert!(v.pan.is_finite());
    }

    /// Cards scale with the zoom, and hit-testing follows them — otherwise a zoomed-out
    /// graph would be unclickable where it is drawn.
    #[test]
    fn hit_test_follows_zoom_and_pan() {
        let v = View {
            pan: Vec2::new(-120.0, 40.0),
            zoom: 0.5,
            ..View::default()
        };
        let cards = layout(names(4).iter().map(String::as_str), area(), &v);
        assert!(
            (cards[0].size.x - CARD_W * 0.5).abs() < 1e-3,
            "cards scale with zoom"
        );
        for (i, c) in cards.iter().enumerate() {
            assert_eq!(hit_test(&cards, c.center()), Some(i));
        }
        // And a drop lands where the pointer is: placing a card at a screen point and
        // laying out again puts it back under that point.
        let mut v2 = v.clone();
        let target = Vec2::new(800.0, 300.0);
        v2.placed
            .insert("State 1".into(), v2.to_local(area(), target));
        assert!(
            (layout(names(4).iter().map(String::as_str), area(), &v2)[1].pos - target).length()
                < 1e-3
        );
    }

    #[test]
    fn edge_points_start_on_the_borders_not_the_centres() {
        let a = CardRect {
            pos: Vec2::new(0.0, 0.0),
            size: Vec2::new(200.0, 100.0),
        };
        let b = CardRect {
            pos: Vec2::new(400.0, 0.0),
            size: Vec2::new(200.0, 100.0),
        };
        let (p, q) = edge_points(a, b);
        // Horizontal neighbours: the edge leaves a's right border and hits b's left.
        assert!((p.x - 200.0).abs() < 0.01, "left card's right border");
        assert!((q.x - 400.0).abs() < 0.01, "right card's left border");
        assert!(
            (p.y - 50.0).abs() < 0.01 && (q.y - 50.0).abs() < 0.01,
            "mid-height"
        );
        // And they are strictly inside the span between the centres.
        assert!(p.x > a.center().x && q.x < b.center().x);
    }

    /// Edges are 1–2px quads, so picking one needs a grab radius far wider than what is
    /// drawn — and where several run close together, the NEAREST must win rather than the
    /// first, or overlapping edges would be unselectable.
    #[test]
    fn edges_pick_by_proximity_within_the_grab_radius() {
        let a = (1u32, Vec2::new(0.0, 100.0), Vec2::new(400.0, 100.0));
        let b = (2u32, Vec2::new(0.0, 120.0), Vec2::new(400.0, 120.0));
        let edges = [a, b];

        assert_eq!(
            hit_edge(&edges, Vec2::new(200.0, 100.0)),
            Some(1),
            "dead on the first"
        );
        assert_eq!(
            hit_edge(&edges, Vec2::new(200.0, 118.0)),
            Some(2),
            "nearest wins"
        );
        assert_eq!(hit_edge(&edges, Vec2::new(200.0, 104.0)), Some(1));
        // Beyond the grab radius, and past the ends, nothing is picked.
        assert_eq!(
            hit_edge(&edges, Vec2::new(200.0, 160.0)),
            None,
            "too far away"
        );
        assert_eq!(
            hit_edge(&edges, Vec2::new(-50.0, 100.0)),
            None,
            "past the start"
        );
        assert_eq!(
            hit_edge(&edges, Vec2::new(460.0, 100.0)),
            None,
            "past the end"
        );
        assert_eq!(hit_edge::<u32>(&[], Vec2::ZERO), None);

        // The tolerance is real: a click a few pixels off a thin line still lands.
        assert_eq!(
            hit_edge(&[a], Vec2::new(200.0, 100.0 + EDGE_GRAB - 0.5)),
            Some(1)
        );
        assert_eq!(
            hit_edge(&[a], Vec2::new(200.0, 100.0 + EDGE_GRAB + 0.5)),
            None
        );
    }

    #[test]
    fn distance_to_a_degenerate_segment_is_the_point_distance() {
        let p = Vec2::new(3.0, 4.0);
        assert!((dist_to_segment(p, Vec2::ZERO, Vec2::ZERO) - 5.0).abs() < 1e-4);
        // Endpoints, not the infinite line: a point beyond the end measures to the end.
        let d = dist_to_segment(Vec2::new(20.0, 0.0), Vec2::ZERO, Vec2::new(10.0, 0.0));
        assert!((d - 10.0).abs() < 1e-4, "clamped to the segment, got {d}");
    }

    #[test]
    fn edge_points_are_stable_for_coincident_cards() {
        let a = CardRect {
            pos: Vec2::ZERO,
            size: Vec2::new(100.0, 50.0),
        };
        let (p, q) = edge_points(a, a);
        assert_eq!(p, a.center());
        assert_eq!(q, a.center());
    }
}
