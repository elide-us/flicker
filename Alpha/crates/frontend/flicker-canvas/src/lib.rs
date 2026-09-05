//! **flicker-canvas** — the node-graph and lane-timeline SURFACE FILLERS.
//!
//! Two editor surfaces that no walker component can express — a graph of draggable
//! cards joined by edges, and a frame-ruled strip of event lanes — packaged the way
//! [`flicker_globe::WorldMap`](https://docs.rs/) and `flicker_rigview::RigView` are:
//! one struct per `surface` panel, seated in a rect the walker reserved, fed content
//! by its consumer every frame, driven by pointer SAMPLES, drawing into a
//! [`HudCommand`](flicker::script::HudCommand) list.
//!
//! # The three contracts every filler here honours
//!
//! 1. **Geometry comes from the seat.** The consumer hands over the walker-reserved
//!    rect ([`GraphCanvas::seat`] / [`Timeline::seat`]); nothing in here ever reads
//!    the screen size, so the filler's picking and its drawing cannot disagree with
//!    the layout the walker actually produced.
//! 2. **Input is a SAMPLE, never a device.** [`PointerSample`] is the walker's
//!    [`SurfacePointer`](flicker::ui::SurfacePointer) shape; a filler never sees an
//!    `InputState`, a key or a binding. The consumer decides which presses and wheel
//!    notches are the filler's to act on (it owns the `hud_hit` gate and knows which
//!    of its own rails the pointer is over) and says so in the sample.
//! 3. **Colour belongs to the consumer.** [`CanvasStyle`] and [`TimelineStyle`] are
//!    nothing but colour slots the consumer fills from `ui_theme.json` tokens. There
//!    is not one rgba literal in this crate — `blank()` zeroes every slot, which is
//!    "unset", not a colour choice, and a filler drawn with it is invisible.
//!
//! Content is plain borrowed structs the consumer rebuilds each frame from its own
//! document ([`GraphNode`] / [`GraphEdge`], [`TimelineLane`] / [`TimelineEvent`]) —
//! not a trait like `MapContent`, because neither filler owns any part of the data:
//! a graph card has no baking, no topology and no projection of its own.

mod graph;
mod timeline;

pub use graph::{
    CanvasEvents, CanvasMetrics, CanvasMode, CanvasStyle, EdgeInk, GraphCanvas, GraphEdge,
    GraphNode, Link, Press, View,
};
pub use timeline::{
    EventGrab, LaneStyle, Timeline, TimelineEdit, TimelineEvent, TimelineEvents, TimelineLane,
    TimelineMetrics, TimelineStyle,
};

use flicker::render::Vec2;

/// **The pointer sample a filler acts on** — the walker's
/// [`SurfacePointer`](flicker::ui::SurfacePointer) reduced to what a 2D editor
/// surface needs, plus the two facts only the consumer can know.
///
/// `pressed` and `wheel` are **consumer-gated**: the scene already knows whether the
/// chrome claimed this frame's pointer (`hud_hit`) and whether one of its own rails
/// wants the wheel, so it zeroes what the filler must not act on rather than the
/// filler re-deriving a gate it has no information for. `left` and `middle` are the
/// raw held states — a drag that began inside the filler must keep tracking even
/// when the cursor wanders over a panel, exactly as the walker's own capture does.
///
/// There is deliberately **no release edge**: each filler latches `left` itself, so a
/// consumer cannot forget to publish one (a dropped release would strand a link drag
/// mid-air, which is precisely the bug this shape prevents).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PointerSample {
    /// Cursor in SCREEN pixels — the same space the seat rect is in.
    pub cursor: Vec2,
    /// Left button held.
    pub left: bool,
    /// Middle button held (pan). Absent from `SurfacePointer`, which carries only
    /// left/right, so a consumer converting one publishes this itself.
    pub middle: bool,
    /// A left PRESS edge the filler may act on — already cleared of presses the
    /// chrome consumed or that landed outside the filler's business.
    pub pressed: bool,
    /// Wheel notches the filler may act on (positive = up), likewise gated.
    pub wheel: f32,
    /// The cursor is inside the filler's seat this frame.
    pub inside: bool,
}

impl From<&flicker::ui::SurfacePointer> for PointerSample {
    /// A walker surface's own sample. The walker only produces one for the surface
    /// that is hot or captured, so `inside` is true by construction; `middle` is
    /// false because `SurfacePointer` does not carry a middle button — a consumer
    /// that pans on middle-drag fills it in after converting.
    fn from(p: &flicker::ui::SurfacePointer) -> Self {
        Self {
            cursor: p.cursor,
            left: p.left,
            middle: false,
            pressed: p.pressed,
            wheel: p.wheel,
            inside: true,
        }
    }
}

/// Whether `p` is inside `r` (inclusive of its far edges, matching every other
/// pick in the engine: a click exactly on a border hits the thing it borders).
pub(crate) fn rect_contains(r: flicker::render::Rect, p: Vec2) -> bool {
    p.x >= r.pos.x && p.x <= r.pos.x + r.size.x && p.y >= r.pos.y && p.y <= r.pos.y + r.size.y
}
