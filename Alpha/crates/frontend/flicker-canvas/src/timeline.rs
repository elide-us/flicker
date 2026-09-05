//! **`Timeline`** — the lane-strip editor surface.
//!
//! A frame ruler over N event lanes: each lane a labelled track, each event either a
//! SPAN (it has an end) or a POINT (it does not), with a playhead sweeping across.
//! The point-vs-span distinction comes from the DATA, never from which lane an event
//! sits on, so one strip serves an animation's hit windows, a wave schedule and a
//! session's event log without a variant per use.
//!
//! Everything is a pure function of the seat rect, the lane count and the frame axis
//! — mapping frames to pixels, placing lanes and bars, picking, snapping a drag back
//! onto whole frames — so all of it is unit-tested without a GPU.

use flicker::render::{Rect, Vec2};
use flicker::script::HudCommand;

use crate::graph::{line, panel, text};
use crate::PointerSample;

/// Strip geometry and interaction distances — design values with a working
/// [`Default`]; a consumer overrides only what its design differs on.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TimelineMetrics {
    /// Width of the lane-label gutter, left of the track area.
    pub gutter: f32,
    /// The frame ruler's height, above the lanes.
    pub ruler_h: f32,
    /// Vertical gap between lane tracks (and above the first / below the last).
    pub lane_gap: f32,
    /// A POINT event (no end frame) draws this wide instead of as a span.
    pub point_w: f32,
    /// How far either side of a bar the pointer may grab it — a 5px marker has to
    /// stay clickable.
    pub grab: f32,
    /// How near a span's edge a press means "resize" rather than "move".
    pub edge_grab: f32,
    /// Zoom floor (1 = the whole axis fits the track) and ceiling.
    pub zoom_min: f32,
    pub zoom_max: f32,
    /// Zoom factor per wheel notch.
    pub zoom_step: f32,
    /// Ruler: label size, its offset right of the tick, and the tick mark's length.
    pub tick_size: f32,
    pub tick_dx: f32,
    pub tick_h: f32,
    /// Lane label: size, its inset from the strip's left edge, the gap after the
    /// colour chip, and the baseline nudge that centres it on the chip.
    pub label_size: f32,
    pub gutter_pad: f32,
    pub label_gap: f32,
    pub label_dy: f32,
    /// The lane's colour chip: a fraction of the lane height, bounded.
    pub chip_frac: f32,
    pub chip_min: f32,
    pub chip_max: f32,
    /// Corner radii: lane rows, event bars, the chip.
    pub row_radius: f32,
    pub event_radius: f32,
    pub chip_radius: f32,
    /// Line widths: lane row border, ruler ticks, the selected-event outline, and
    /// the playhead.
    pub border: f32,
    pub tick_w: f32,
    pub outline: f32,
    pub playhead_w: f32,
}

impl Default for TimelineMetrics {
    fn default() -> Self {
        Self {
            gutter: 120.0,
            ruler_h: 15.0,
            lane_gap: 2.0,
            point_w: 5.0,
            grab: 4.0,
            edge_grab: 3.0,
            zoom_min: 1.0,
            zoom_max: 16.0,
            zoom_step: 0.12,
            tick_size: 10.0,
            tick_dx: 3.0,
            tick_h: 4.0,
            label_size: 10.0,
            gutter_pad: 8.0,
            label_gap: 6.0,
            label_dy: -1.0,
            chip_frac: 0.4,
            chip_min: 4.0,
            chip_max: 9.0,
            row_radius: 3.0,
            event_radius: 2.0,
            chip_radius: 1.0,
            border: 1.0,
            tick_w: 1.0,
            outline: 1.0,
            playhead_w: 2.0,
        }
    }
}

/// One lane's four colours — filled by the CONSUMER from its theme tokens, so a
/// lane's meaning (a hitbox window, a spawn wave) and its ink stay in the one place
/// that knows both.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct LaneStyle {
    pub row: [f32; 4],
    pub row_border: [f32; 4],
    /// The gutter chip AND the label beside it.
    pub swatch: [f32; 4],
    /// The lane's event bars.
    pub event: [f32; 4],
}

/// The strip's own colours, likewise consumer-filled. There are no colour literals
/// in this crate; [`TimelineStyle::blank`] leaves every slot transparent.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct TimelineStyle {
    /// Ruler frame numbers.
    pub ruler: [f32; 4],
    /// Ruler tick marks.
    pub tick: [f32; 4],
    pub playhead: [f32; 4],
    /// The outline around the selected event bar — an outline rather than a fill so
    /// the bar's own lane colour still reads.
    pub event_selected: [f32; 4],
}

impl TimelineStyle {
    pub fn blank() -> Self {
        Self::default()
    }
}

/// One lane, top to bottom in the order given.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct TimelineLane<'a> {
    pub label: &'a str,
    pub style: LaneStyle,
}

/// One authored event. `end` absent = a one-shot, drawn as a fixed-width marker.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct TimelineEvent {
    pub lane: usize,
    pub start: u32,
    pub end: Option<u32>,
    pub selected: bool,
}

/// Which part of a bar a drag has hold of.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EventGrab {
    /// The body — both ends move together, the span keeping its length.
    Move,
    /// The left edge.
    Start,
    /// The right edge (spans only).
    End,
}

/// The event's new frames while a drag is live — ABSOLUTE and already snapped to
/// whole frames and clamped into the axis, so a consumer applies them straight onto
/// its document without re-deriving anything.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TimelineEdit {
    pub event: usize,
    pub grab: EventGrab,
    pub start: u32,
    pub end: Option<u32>,
}

/// A press the strip resolved: the event under it (if any), the lane it landed in,
/// and the frame it snapped to.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TimelinePress {
    pub event: Option<usize>,
    pub lane: Option<usize>,
    pub frame: u32,
}

/// What one [`Timeline::pointer`] pass produced.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct TimelineEvents {
    pub pressed: Option<TimelinePress>,
    pub edit: Option<TimelineEdit>,
}

/// A live drag: what was grabbed, from where, and the values it started with.
#[derive(Clone, Copy, Debug)]
struct Grab {
    event: usize,
    kind: EventGrab,
    origin: i64,
    start: u32,
    end: Option<u32>,
    /// The pointer has actually crossed a frame boundary — until it has, a press is
    /// a click and must not dirty the consumer's document.
    moved: bool,
}

/// **The lane-timeline surface filler.** Seat it in the walker-reserved rect with
/// this frame's lane count and frame axis, hand it the pointer sample, then draw.
pub struct Timeline {
    metrics: TimelineMetrics,
    strip: Rect,
    lanes: usize,
    frames: u32,
    /// First visible frame — `0` while the whole axis fits.
    scroll: f32,
    /// How many times the axis is magnified; `1` = the whole axis across the track.
    zoom: f32,
    playhead: u32,
    grab: Option<Grab>,
    pan_from: Option<Vec2>,
    prev_left: bool,
}

impl Timeline {
    pub fn new(metrics: TimelineMetrics) -> Self {
        Self {
            metrics,
            strip: Rect {
                pos: Vec2::ZERO,
                size: Vec2::ZERO,
            },
            lanes: 0,
            frames: 0,
            scroll: 0.0,
            zoom: 1.0,
            playhead: 0,
            grab: None,
            pan_from: None,
            prev_left: false,
        }
    }

    /// Seat the strip: the walker-reserved rect, how many lanes it shows, and the
    /// frame axis it spans. All three together, every frame, because the three are
    /// what every other method here derives from — a stale one of them is a strip
    /// that draws in a different place than it picks.
    pub fn seat(&mut self, strip: Rect, lanes: usize, frames: u32) {
        self.strip = strip;
        self.lanes = lanes;
        self.frames = frames;
        self.clamp_scroll();
    }

    pub fn strip(&self) -> Rect {
        self.strip
    }

    pub fn metrics(&self) -> &TimelineMetrics {
        &self.metrics
    }

    pub fn frames(&self) -> u32 {
        self.frames
    }

    pub fn zoom(&self) -> f32 {
        self.zoom
    }

    pub fn scroll(&self) -> f32 {
        self.scroll
    }

    /// Where the playhead stands. The consumer owns the clock — this only draws it.
    pub fn set_playhead(&mut self, frame: u32) {
        self.playhead = frame;
    }

    pub fn playhead(&self) -> u32 {
        self.playhead
    }

    /// Move the visible window by whole frames (a scrollbar, a keyboard nudge).
    pub fn scroll_by(&mut self, frames: f32) {
        self.scroll += frames;
        self.clamp_scroll();
    }

    /// Forget the zoom and the scroll — a new clip is a new axis, not the previous
    /// one's window over different frames.
    pub fn reset_view(&mut self) {
        self.zoom = 1.0;
        self.scroll = 0.0;
        self.grab = None;
        self.pan_from = None;
    }

    /// Left edge of the track area — right of the label gutter.
    pub fn track_x(&self) -> f32 {
        self.strip.pos.x + self.metrics.gutter
    }

    /// Width of the track area. Never zero, so frame→pixel can't divide by nothing.
    pub fn track_w(&self) -> f32 {
        (self.strip.size.x - self.metrics.gutter).max(1.0)
    }

    /// How many frames the track shows at the current zoom. Never zero.
    pub fn visible(&self) -> f32 {
        (self.frames.max(1) as f32 / self.zoom).max(1.0)
    }

    /// Height of one lane track. The lanes share whatever is left under the ruler, so
    /// the strip stays correct at any height rather than only at the mock's.
    pub fn lane_h(&self) -> f32 {
        let n = self.lanes.max(1) as f32;
        ((self.strip.size.y - self.metrics.ruler_h - self.metrics.lane_gap * (n + 1.0)) / n)
            .max(1.0)
    }

    /// A lane track's rect.
    pub fn lane_rect(&self, index: usize) -> Rect {
        let h = self.lane_h();
        Rect {
            pos: Vec2::new(
                self.track_x(),
                self.strip.pos.y
                    + self.metrics.ruler_h
                    + self.metrics.lane_gap
                    + index as f32 * (h + self.metrics.lane_gap),
            ),
            size: Vec2::new(self.track_w(), h),
        }
    }

    /// The lane `y` falls in, if any.
    pub fn lane_at(&self, y: f32) -> Option<usize> {
        (0..self.lanes).find(|i| {
            let r = self.lane_rect(*i);
            y >= r.pos.y && y <= r.pos.y + r.size.y
        })
    }

    /// Where a frame sits along the track. Clamped into the strip, so an event
    /// authored past the end of its clip is still visible at the edge rather than
    /// drawn off-screen.
    pub fn frame_x(&self, frame: u32) -> f32 {
        let t = if self.frames == 0 {
            0.0
        } else {
            ((frame as f32 - self.scroll) / self.visible()).clamp(0.0, 1.0)
        };
        self.track_x() + t * self.track_w()
    }

    /// The frame under `x`, SNAPPED to a whole frame and held inside the axis. The
    /// snap is the whole point: an authored tick is an integer, so a drag that
    /// produced 12.4 would be a lie about what was authored.
    pub fn frame_at(&self, x: f32) -> u32 {
        self.frame_at_f32(x).round().clamp(0.0, self.frames as f32) as u32
    }

    /// The unsnapped frame under `x` — the anchored-zoom pivot.
    fn frame_at_f32(&self, x: f32) -> f32 {
        self.scroll + (x - self.track_x()) / self.track_w() * self.visible()
    }

    /// An event's bar.
    pub fn event_rect(&self, ev: &TimelineEvent) -> Rect {
        let track = self.lane_rect(ev.lane);
        let x0 = self.frame_x(ev.start);
        let w = match ev.end {
            Some(e) => (self.frame_x(e.max(ev.start)) - x0).max(self.metrics.point_w),
            None => self.metrics.point_w,
        };
        // Keep the bar inside the track even when it starts at the very last frame.
        let x0 = x0.min(track.pos.x + track.size.x - w);
        Rect {
            pos: Vec2::new(x0, track.pos.y),
            size: Vec2::new(w, track.size.y),
        }
    }

    /// The event under `p` — within its lane, and within the grab radius either side
    /// of its bar. Nearest bar centre wins, so overlapping windows resolve to the one
    /// actually under the cursor.
    pub fn event_at(&self, p: Vec2, events: &[TimelineEvent]) -> Option<usize> {
        events
            .iter()
            .enumerate()
            .filter_map(|(i, ev)| {
                let bar = self.event_rect(ev);
                let within_y = p.y >= bar.pos.y && p.y <= bar.pos.y + bar.size.y;
                let x0 = bar.pos.x - self.metrics.grab;
                let x1 = bar.pos.x + bar.size.x + self.metrics.grab;
                (within_y && p.x >= x0 && p.x <= x1)
                    .then(|| (i, (p.x - (bar.pos.x + bar.size.x * 0.5)).abs()))
            })
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(i, _)| i)
    }

    /// Ruler tick frames across the VISIBLE window: a round step that keeps the label
    /// count readable at any clip length or zoom (a 40-frame clip unzoomed yields
    /// 0,5,…,40 — nine ticks).
    pub fn ruler_ticks(&self) -> Vec<u32> {
        if self.frames == 0 {
            return vec![0];
        }
        const STEPS: [u32; 9] = [1, 2, 5, 10, 15, 20, 30, 60, 120];
        let span = self.visible().ceil().max(1.0) as u32;
        let step = STEPS
            .iter()
            .copied()
            .find(|s| span / s <= 10)
            .unwrap_or(span.max(1));
        let first = (self.scroll.max(0.0) / step as f32).ceil() as u32 * step;
        let last = (self.scroll + self.visible()).min(self.frames as f32) as u32;
        (first..=last.max(first))
            .step_by(step as usize)
            .filter(|f| *f <= self.frames)
            .collect()
    }

    fn clamp_scroll(&mut self) {
        let max = (self.frames as f32 - self.visible()).max(0.0);
        self.scroll = self.scroll.clamp(0.0, max);
    }

    /// **Apply this frame's pointer sample.** Wheel zooms about the frame under the
    /// cursor, middle-drag scrolls the window, a press picks (and takes hold of) an
    /// event bar, and a held drag reports the event's new frames.
    pub fn pointer(&mut self, sample: &PointerSample, events: &[TimelineEvent]) -> TimelineEvents {
        let mut out = TimelineEvents::default();

        // Anchored zoom: the frame under the cursor stays under the cursor.
        if sample.wheel != 0.0 && sample.inside {
            let anchor = self.frame_at_f32(sample.cursor.x);
            self.zoom = (self.zoom * (1.0 + sample.wheel * self.metrics.zoom_step))
                .clamp(self.metrics.zoom_min, self.metrics.zoom_max);
            let t = ((sample.cursor.x - self.track_x()) / self.track_w()).clamp(0.0, 1.0);
            self.scroll = anchor - t * self.visible();
            self.clamp_scroll();
        }

        // Middle-drag scrolls, in frames, so panning never competes with a bar drag.
        if sample.middle {
            if let Some(from) = self.pan_from {
                let dx = sample.cursor.x - from.x;
                self.scroll -= dx / self.track_w() * self.visible();
                self.clamp_scroll();
            }
            self.pan_from = Some(sample.cursor);
        } else {
            self.pan_from = None;
        }

        if sample.pressed {
            let event = self.event_at(sample.cursor, events);
            self.grab = event.and_then(|i| {
                let ev = events.get(i)?;
                let bar = self.event_rect(ev);
                let kind = if ev.end.is_none() {
                    EventGrab::Move
                } else if sample.cursor.x <= bar.pos.x + self.metrics.edge_grab {
                    EventGrab::Start
                } else if sample.cursor.x >= bar.pos.x + bar.size.x - self.metrics.edge_grab {
                    EventGrab::End
                } else {
                    EventGrab::Move
                };
                Some(Grab {
                    event: i,
                    kind,
                    origin: self.frame_at(sample.cursor.x) as i64,
                    start: ev.start,
                    end: ev.end,
                    moved: false,
                })
            });
            out.pressed = Some(TimelinePress {
                event,
                lane: self.lane_at(sample.cursor.y),
                frame: self.frame_at(sample.cursor.x),
            });
        }

        if self.grab.is_some() {
            let here = self.frame_at(sample.cursor.x) as i64;
            let frames = self.frames;
            if sample.left {
                if let Some(g) = self.grab.as_mut() {
                    let delta = here - g.origin;
                    g.moved |= delta != 0;
                    if g.moved {
                        out.edit = Some(edit(g, delta, frames));
                    }
                }
            } else {
                self.grab = None;
            }
        }
        // A release with no button ever held (a stray sample) must still not strand a
        // grab: the latch below is what the next press reads.
        if self.prev_left && !sample.left {
            self.grab = None;
        }
        self.prev_left = sample.left;
        out
    }

    /// **Draw the strip** into `out` at `layer`: ruler, lane rows with their gutter
    /// chips and labels, the event bars, and the playhead.
    ///
    /// The strip's own backdrop is the CONSUMER's — it usually spans more than the
    /// lanes (a header line, a title) and belongs to the panel the surface sits in.
    pub fn draw(
        &self,
        lanes: &[TimelineLane],
        events: &[TimelineEvent],
        style: &TimelineStyle,
        layer: f32,
        out: &mut Vec<HudCommand>,
    ) {
        let m = &self.metrics;
        for f in self.ruler_ticks() {
            let x = self.frame_x(f);
            out.push(text(
                &f.to_string(),
                Vec2::new(x + m.tick_dx, self.strip.pos.y),
                m.tick_size,
                style.ruler,
                layer,
            ));
            out.push(line(
                Vec2::new(x, self.strip.pos.y + m.ruler_h - m.tick_h),
                Vec2::new(x, self.strip.pos.y + m.ruler_h),
                m.tick_w,
                style.tick,
                layer,
            ));
        }

        for (i, lane) in lanes.iter().enumerate() {
            let r = self.lane_rect(i);
            out.push(panel(
                r,
                lane.style.row,
                lane.style.row,
                0.0,
                m.row_radius,
                m.border,
                lane.style.row_border,
                layer,
            ));
            // The gutter's colour chip, then the label beside it.
            let chip = (r.size.y * m.chip_frac).clamp(m.chip_min, m.chip_max);
            let cy = r.pos.y + (r.size.y - chip) * 0.5;
            out.push(panel(
                Rect {
                    pos: Vec2::new(self.strip.pos.x + m.gutter_pad, cy),
                    size: Vec2::splat(chip),
                },
                lane.style.swatch,
                lane.style.swatch,
                0.0,
                m.chip_radius,
                0.0,
                lane.style.swatch,
                layer,
            ));
            out.push(text(
                lane.label,
                Vec2::new(
                    self.strip.pos.x + m.gutter_pad + chip + m.label_gap,
                    cy + m.label_dy,
                ),
                m.label_size,
                lane.style.swatch,
                layer,
            ));
        }

        for ev in events {
            let fill = lanes
                .get(ev.lane)
                .map(|l| l.style.event)
                .unwrap_or_default();
            let bar = self.event_rect(ev);
            out.push(panel(
                bar,
                fill,
                fill,
                0.0,
                m.event_radius,
                0.0,
                fill,
                layer,
            ));
            if ev.selected {
                // An outline, not a fill: the bar's own lane colour still has to read.
                let tr = Vec2::new(bar.pos.x + bar.size.x, bar.pos.y);
                let bl = Vec2::new(bar.pos.x, bar.pos.y + bar.size.y);
                let br = bar.pos + bar.size;
                for (a, b) in [(bar.pos, tr), (tr, br), (br, bl), (bl, bar.pos)] {
                    out.push(line(a, b, m.outline, style.event_selected, layer));
                }
            }
        }

        if self.frames > 0 {
            let x = self.frame_x(self.playhead);
            out.push(line(
                Vec2::new(x, self.strip.pos.y),
                Vec2::new(x, self.strip.pos.y + self.strip.size.y),
                m.playhead_w,
                style.playhead,
                layer,
            ));
        }
    }
}

/// The dragged event's new frames. Clamped as a WHOLE: a move keeps the span's
/// length and stops at the axis ends rather than squashing against them; a resize
/// stops at its own other end rather than inverting through it.
fn edit(g: &Grab, delta: i64, frames: u32) -> TimelineEdit {
    let max = frames as i64;
    let (start, end) = match g.kind {
        EventGrab::Move => {
            let far = g.end.unwrap_or(g.start) as i64;
            let d = delta.clamp(-(g.start as i64), max - far);
            (g.start as i64 + d, g.end.map(|e| e as i64 + d))
        }
        EventGrab::Start => {
            let ceiling = g.end.map(i64::from).unwrap_or(max);
            (
                (g.start as i64 + delta).clamp(0, ceiling),
                g.end.map(i64::from),
            )
        }
        EventGrab::End => (
            g.start as i64,
            g.end.map(|e| (e as i64 + delta).clamp(g.start as i64, max)),
        ),
    };
    TimelineEdit {
        event: g.event,
        grab: g.kind,
        start: start as u32,
        end: end.map(|e| e as u32),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LANES: usize = 9;

    fn strip() -> Rect {
        Rect {
            pos: Vec2::new(0.0, 900.0),
            size: Vec2::new(1920.0, 180.0),
        }
    }

    fn seated(frames: u32) -> Timeline {
        let mut t = Timeline::new(TimelineMetrics::default());
        t.seat(strip(), LANES, frames);
        t
    }

    fn sample(cursor: Vec2) -> PointerSample {
        PointerSample {
            cursor,
            inside: true,
            ..PointerSample::default()
        }
    }

    fn span(lane: usize, start: u32, end: u32) -> TimelineEvent {
        TimelineEvent {
            lane,
            start,
            end: Some(end),
            selected: false,
        }
    }

    fn point(lane: usize, start: u32) -> TimelineEvent {
        TimelineEvent {
            lane,
            start,
            end: None,
            selected: false,
        }
    }

    /// The lanes tile the strip below the ruler, in order, without overlapping.
    #[test]
    fn lanes_stack_in_order_inside_the_strip() {
        let t = seated(40);
        let rects: Vec<Rect> = (0..LANES).map(|i| t.lane_rect(i)).collect();
        for w in rects.windows(2) {
            assert!(w[1].pos.y > w[0].pos.y, "lanes descend");
            assert!(
                w[1].pos.y >= w[0].pos.y + w[0].size.y,
                "lanes must not overlap"
            );
        }
        let (first, last) = (rects[0], rects[LANES - 1]);
        assert!(
            first.pos.y >= strip().pos.y + TimelineMetrics::default().ruler_h,
            "lanes start below the ruler"
        );
        assert!(
            last.pos.y + last.size.y <= strip().pos.y + strip().size.y + 0.01,
            "the last lane stays inside the strip"
        );
        assert!(
            first.pos.x >= strip().pos.x + TimelineMetrics::default().gutter,
            "tracks clear the label gutter"
        );
        // And the lane under a point is the lane that contains it.
        for (i, r) in rects.iter().enumerate() {
            assert_eq!(t.lane_at(r.pos.y + r.size.y * 0.5), Some(i));
        }
        assert_eq!(t.lane_at(strip().pos.y), None, "the ruler is not a lane");
    }

    /// A cramped strip must still produce usable, positive geometry rather than
    /// negative heights that would draw inverted quads.
    #[test]
    fn lane_geometry_survives_a_tiny_strip() {
        let mut t = Timeline::new(TimelineMetrics::default());
        t.seat(
            Rect {
                pos: Vec2::ZERO,
                size: Vec2::new(40.0, 10.0),
            },
            LANES,
            40,
        );
        assert!(t.lane_h() > 0.0);
        assert!(t.track_w() > 0.0);
        let r = t.lane_rect(6);
        assert!(r.size.x > 0.0 && r.size.y > 0.0);
        // Zero lanes is a strip with nothing on it, not a divide-by-zero.
        t.seat(strip(), 0, 40);
        assert!(t.lane_h() > 0.0);
        assert_eq!(t.lane_at(strip().pos.y + 40.0), None);
    }

    #[test]
    fn frames_map_across_the_track_and_clamp() {
        let t = seated(40);
        assert!(
            (t.frame_x(0) - t.track_x()).abs() < 0.01,
            "frame 0 at the left"
        );
        assert!(
            (t.frame_x(40) - (t.track_x() + t.track_w())).abs() < 0.01,
            "the last frame reaches the right"
        );
        assert!(t.frame_x(20) > t.frame_x(10), "frames advance rightward");
        // A zero-length clip and an over-long event are both survivable.
        assert!(seated(0).frame_x(5).is_finite());
        assert!(
            (t.frame_x(999) - (t.track_x() + t.track_w())).abs() < 0.01,
            "clamped"
        );
    }

    /// Pixels snap back to WHOLE frames, and the round-trip holds — an authored tick
    /// is an integer, so a drag that produced 12.4 would misreport what was authored.
    #[test]
    fn pixels_snap_back_onto_whole_frames() {
        let t = seated(40);
        for f in [0u32, 1, 17, 39, 40] {
            assert_eq!(t.frame_at(t.frame_x(f)), f, "frame {f} lost its identity");
        }
        // Half a frame either side of a tick still lands on it.
        let step = t.track_w() / 40.0;
        assert_eq!(t.frame_at(t.frame_x(10) + step * 0.4), 10);
        assert_eq!(t.frame_at(t.frame_x(10) - step * 0.4), 10);
        assert_eq!(t.frame_at(t.frame_x(10) + step * 0.6), 11);
        // Off either end of the track clamps into the axis rather than wrapping.
        assert_eq!(t.frame_at(t.track_x() - 500.0), 0);
        assert_eq!(t.frame_at(t.track_x() + t.track_w() + 500.0), 40);
    }

    /// A window event spans its frames; a one-shot draws as a marker. Both stay
    /// inside their lane, including one authored on the very last frame.
    #[test]
    fn event_bars_span_windows_and_mark_one_shots() {
        let t = seated(40);
        let s = t.event_rect(&span(0, 10, 20));
        let p = t.event_rect(&point(0, 10));
        assert!(s.size.x > p.size.x, "a window is wider than a one-shot");
        assert_eq!(p.size.x, TimelineMetrics::default().point_w);
        assert_eq!(s.pos.y, t.lane_rect(0).pos.y, "the bar sits in its lane");
        assert_eq!(s.size.y, t.lane_rect(0).size.y);

        // A zero-length window is still visible, not a hairline.
        assert_eq!(
            t.event_rect(&span(0, 10, 10)).size.x,
            TimelineMetrics::default().point_w
        );
        // An inverted window (end before start) does not produce a negative width.
        assert!(t.event_rect(&span(0, 30, 5)).size.x > 0.0);
        // An event on the last frame is pulled back inside the track.
        let edge = t.event_rect(&point(0, 40));
        let track = t.lane_rect(0);
        assert!(
            edge.pos.x + edge.size.x <= track.pos.x + track.size.x + 0.01,
            "an end-of-clip marker stays inside the track"
        );
    }

    #[test]
    fn ruler_ticks_stay_readable_at_any_clip_length() {
        assert_eq!(
            seated(40).ruler_ticks(),
            vec![0, 5, 10, 15, 20, 25, 30, 35, 40]
        );
        for frames in [0, 1, 7, 40, 120, 600, 3600] {
            let t = seated(frames);
            let ticks = t.ruler_ticks();
            assert!(!ticks.is_empty(), "{frames}: always at least frame 0");
            assert!(
                ticks.len() <= 11,
                "{frames}: {} ticks is unreadable",
                ticks.len()
            );
            assert_eq!(ticks[0], 0);
            assert!(
                ticks.windows(2).all(|w| w[1] > w[0]),
                "{frames}: ticks ascend"
            );
            assert!(
                ticks.iter().all(|f| *f <= frames.max(1)),
                "{frames}: tick past the clip"
            );
        }
    }

    /// Zoomed in, the ruler labels the VISIBLE window rather than the whole axis —
    /// otherwise a magnified strip would show one tick or none at all.
    #[test]
    fn the_ruler_follows_the_zoomed_window() {
        let mut t = seated(600);
        t.zoom = 10.0;
        t.scroll_by(300.0);
        let ticks = t.ruler_ticks();
        assert!(!ticks.is_empty() && ticks.len() <= 11);
        assert!(ticks[0] >= 300, "starts inside the window, got {ticks:?}");
        assert!(
            *ticks.last().unwrap() <= 300 + t.visible().ceil() as u32,
            "ends inside the window, got {ticks:?}"
        );
        assert!(ticks.windows(2).all(|w| w[1] > w[0]));
    }

    /// Wheel zoom is anchored on the frame under the cursor, clamps at both ends,
    /// and never leaves the window off the end of the axis.
    #[test]
    fn wheel_zoom_is_anchored_and_clamped() {
        let mut t = seated(400);
        let cursor = Vec2::new(t.track_x() + t.track_w() * 0.75, t.strip.pos.y + 40.0);
        let before = t.frame_at(cursor.x);
        t.pointer(
            &PointerSample {
                wheel: 3.0,
                ..sample(cursor)
            },
            &[],
        );
        assert!(t.zoom() > 1.0, "zoom actually changed");
        assert!(
            (t.frame_at(cursor.x) as i64 - before as i64).abs() <= 1,
            "the anchor drifted: {} → {}",
            before,
            t.frame_at(cursor.x)
        );

        let m = TimelineMetrics::default();
        for _ in 0..80 {
            t.pointer(
                &PointerSample {
                    wheel: 3.0,
                    ..sample(cursor)
                },
                &[],
            );
        }
        assert!((t.zoom() - m.zoom_max).abs() < 1e-3, "past the ceiling");
        for _ in 0..200 {
            t.pointer(
                &PointerSample {
                    wheel: -3.0,
                    ..sample(cursor)
                },
                &[],
            );
        }
        assert!((t.zoom() - m.zoom_min).abs() < 1e-3, "past the floor");
        assert_eq!(t.scroll(), 0.0, "unzoomed, the whole axis is the window");

        // Outside the strip the wheel is not the timeline's business.
        t.pointer(
            &PointerSample {
                wheel: 3.0,
                inside: false,
                ..sample(cursor)
            },
            &[],
        );
        assert_eq!(t.zoom(), m.zoom_min);
    }

    /// Unzoomed, the window IS the axis: scrolling it is a no-op rather than sliding
    /// the whole clip off the left of the strip.
    #[test]
    fn scrolling_is_clamped_to_the_axis() {
        let mut t = seated(40);
        t.scroll_by(30.0);
        assert_eq!(t.scroll(), 0.0);
        t.zoom = 4.0;
        t.seat(strip(), LANES, 40);
        t.scroll_by(1000.0);
        assert!(
            (t.scroll() - (40.0 - t.visible())).abs() < 1e-3,
            "the last page is reachable and no further, got {}",
            t.scroll()
        );
        t.scroll_by(-1000.0);
        assert_eq!(t.scroll(), 0.0);
    }

    /// Picking: a bar is grabbed within its lane and within the grab radius either
    /// side, and where two overlap the nearest wins.
    #[test]
    fn events_pick_within_their_lane_and_grab_radius() {
        let t = seated(40);
        let events = [span(0, 10, 20), span(0, 18, 30), point(3, 8)];
        let bar = t.event_rect(&events[0]);
        let mid_y = bar.pos.y + bar.size.y * 0.5;
        assert_eq!(
            t.event_at(Vec2::new(t.frame_x(12), mid_y), &events),
            Some(0)
        );
        assert_eq!(
            t.event_at(Vec2::new(t.frame_x(28), mid_y), &events),
            Some(1),
            "past the first bar's end"
        );
        // A 5px marker is still clickable, thanks to the grab radius.
        let p = t.event_rect(&events[2]);
        let py = p.pos.y + p.size.y * 0.5;
        assert_eq!(t.event_at(Vec2::new(p.pos.x - 3.0, py), &events), Some(2));
        assert_eq!(
            t.event_at(Vec2::new(p.pos.x - 40.0, py), &events),
            None,
            "too far"
        );
        // The right lane matters: the same x in another lane hits nothing.
        assert_eq!(
            t.event_at(
                Vec2::new(t.frame_x(12), t.lane_rect(5).pos.y + 2.0),
                &events
            ),
            None
        );
        assert_eq!(t.event_at(Vec2::new(t.frame_x(12), mid_y), &[]), None);
    }

    /// A press reports what it hit, the lane and the snapped frame — and a press on
    /// empty track reports no event, which is how a consumer clears its selection.
    #[test]
    fn a_press_reports_the_event_the_lane_and_the_frame() {
        let mut t = seated(40);
        let events = [span(2, 10, 20)];
        let bar = t.event_rect(&events[0]);
        let at = Vec2::new(t.frame_x(15), bar.pos.y + bar.size.y * 0.5);
        let p = t
            .pointer(
                &PointerSample {
                    pressed: true,
                    left: true,
                    ..sample(at)
                },
                &events,
            )
            .pressed
            .unwrap();
        assert_eq!(p.event, Some(0));
        assert_eq!(p.lane, Some(2));
        assert_eq!(p.frame, 15);

        let empty = Vec2::new(t.frame_x(35), t.lane_rect(6).pos.y + 2.0);
        let p = t
            .pointer(
                &PointerSample {
                    pressed: true,
                    left: true,
                    ..sample(empty)
                },
                &events,
            )
            .pressed
            .unwrap();
        assert_eq!(p.event, None, "empty track clears the selection");
        assert_eq!(p.lane, Some(6));
    }

    /// Dragging a bar's BODY moves both ends together, snapped to whole frames, and
    /// a press that does not travel a whole frame is a click — it must not report an
    /// edit and dirty the consumer's document.
    #[test]
    fn dragging_a_bar_moves_the_whole_span_and_a_click_edits_nothing() {
        let mut t = seated(40);
        let events = [span(1, 10, 20)];
        let bar = t.event_rect(&events[0]);
        let y = bar.pos.y + bar.size.y * 0.5;
        let at = Vec2::new(t.frame_x(15), y);
        let ev = t.pointer(
            &PointerSample {
                pressed: true,
                left: true,
                ..sample(at)
            },
            &events,
        );
        assert!(ev.edit.is_none(), "the press alone edits nothing");

        let ev = t.pointer(
            &PointerSample {
                left: true,
                ..sample(Vec2::new(t.frame_x(20), y))
            },
            &events,
        );
        assert_eq!(
            ev.edit,
            Some(TimelineEdit {
                event: 0,
                grab: EventGrab::Move,
                start: 15,
                end: Some(25),
            }),
            "the span kept its length"
        );

        // Released: further motion is not this event's business any more.
        t.pointer(&sample(Vec2::new(t.frame_x(30), y)), &events);
        assert!(t
            .pointer(&sample(Vec2::new(t.frame_x(35), y)), &events)
            .edit
            .is_none());
    }

    /// A move stops at the axis ends with its length intact rather than squashing
    /// against them.
    #[test]
    fn a_move_stops_at_the_axis_without_squashing() {
        let mut t = seated(40);
        let events = [span(1, 30, 36)];
        let bar = t.event_rect(&events[0]);
        let y = bar.pos.y + bar.size.y * 0.5;
        t.pointer(
            &PointerSample {
                pressed: true,
                left: true,
                ..sample(Vec2::new(t.frame_x(33), y))
            },
            &events,
        );
        let e = t
            .pointer(
                &PointerSample {
                    left: true,
                    ..sample(Vec2::new(t.track_x() + t.track_w() + 200.0, y))
                },
                &events,
            )
            .edit
            .unwrap();
        assert_eq!(
            (e.start, e.end),
            (34, Some(40)),
            "length preserved at the end"
        );
    }

    /// Grabbing an END resizes only that end, and cannot be dragged through the
    /// other one into a negative span.
    #[test]
    fn grabbing_an_edge_resizes_and_never_inverts() {
        let mut t = seated(40);
        let events = [span(4, 10, 20)];
        let bar = t.event_rect(&events[0]);
        let y = bar.pos.y + bar.size.y * 0.5;

        // The right edge.
        t.pointer(
            &PointerSample {
                pressed: true,
                left: true,
                ..sample(Vec2::new(bar.pos.x + bar.size.x - 1.0, y))
            },
            &events,
        );
        let e = t
            .pointer(
                &PointerSample {
                    left: true,
                    ..sample(Vec2::new(t.frame_x(30), y))
                },
                &events,
            )
            .edit
            .unwrap();
        assert_eq!(e.grab, EventGrab::End);
        assert_eq!((e.start, e.end), (10, Some(30)), "only the end moved");
        // Dragged back past the start it stops there rather than inverting.
        let e = t
            .pointer(
                &PointerSample {
                    left: true,
                    ..sample(Vec2::new(t.frame_x(0), y))
                },
                &events,
            )
            .edit
            .unwrap();
        assert_eq!((e.start, e.end), (10, Some(10)));

        // The left edge, on a fresh press.
        t.pointer(&sample(Vec2::new(t.frame_x(0), y)), &events);
        t.pointer(
            &PointerSample {
                pressed: true,
                left: true,
                ..sample(Vec2::new(bar.pos.x + 1.0, y))
            },
            &events,
        );
        let e = t
            .pointer(
                &PointerSample {
                    left: true,
                    ..sample(Vec2::new(t.frame_x(4), y))
                },
                &events,
            )
            .edit
            .unwrap();
        assert_eq!(e.grab, EventGrab::Start);
        assert_eq!((e.start, e.end), (4, Some(20)), "only the start moved");
    }

    /// A POINT event has no edges to resize: however it is grabbed it moves, and it
    /// stays a point.
    #[test]
    fn a_point_event_always_moves_and_stays_a_point() {
        let mut t = seated(40);
        let events = [point(2, 12)];
        let bar = t.event_rect(&events[0]);
        let y = bar.pos.y + bar.size.y * 0.5;
        t.pointer(
            &PointerSample {
                pressed: true,
                left: true,
                ..sample(Vec2::new(bar.pos.x, y))
            },
            &events,
        );
        let e = t
            .pointer(
                &PointerSample {
                    left: true,
                    ..sample(Vec2::new(t.frame_x(25), y))
                },
                &events,
            )
            .edit
            .unwrap();
        assert_eq!(e.grab, EventGrab::Move);
        assert_eq!(e.end, None, "a point stays a point");
        assert!(e.start > 12);
    }

    /// The draw emits the ruler, the lanes, the bars and the playhead, with every
    /// colour taken from the consumer's style — and the selected bar's highlight is
    /// four LINES around it, not a fill that would hide its lane colour.
    #[test]
    fn the_draw_paints_from_the_given_style_only() {
        let mut t = seated(40);
        t.set_playhead(20);
        let lane_style = LaneStyle {
            row: [0.1, 0.0, 0.0, 1.0],
            row_border: [0.2, 0.0, 0.0, 1.0],
            swatch: [0.3, 0.0, 0.0, 1.0],
            event: [0.4, 0.0, 0.0, 1.0],
        };
        let lanes: Vec<TimelineLane> = (0..LANES)
            .map(|_| TimelineLane {
                label: "Hitbox",
                style: lane_style,
            })
            .collect();
        let mut evs = [span(0, 10, 20), point(1, 5)];
        evs[0].selected = true;
        let style = TimelineStyle {
            ruler: [0.0, 1.0, 0.0, 1.0],
            tick: [0.0, 0.9, 0.0, 1.0],
            playhead: [0.0, 0.0, 1.0, 1.0],
            event_selected: [1.0, 1.0, 1.0, 1.0],
        };
        let mut out = Vec::new();
        t.draw(&lanes, &evs, &style, 2.0, &mut out);

        let ticks = t.ruler_ticks().len();
        let texts = out
            .iter()
            .filter(|c| matches!(c, HudCommand::Text { .. }))
            .count();
        assert_eq!(texts, ticks + LANES, "a number per tick, a label per lane");
        // ticks + 4 outline lines + 1 playhead.
        let lines = out
            .iter()
            .filter(|c| matches!(c, HudCommand::Line { .. }))
            .count();
        assert_eq!(lines, ticks + 5);
        assert!(out.iter().any(|c| matches!(
            c,
            HudCommand::Line { color, width, .. }
                if *color == style.playhead && *width == TimelineMetrics::default().playhead_w
        )));
        for cmd in &out {
            let (c, layer) = match cmd {
                HudCommand::Panel { color, layer, .. } => (*color, *layer),
                HudCommand::Line { color, layer, .. } => (*color, *layer),
                HudCommand::Text { color, layer, .. } => (*color, *layer),
                _ => continue,
            };
            assert_eq!(layer, 2.0, "every command carries the caller's layer");
            assert!(
                [
                    lane_style.row,
                    lane_style.swatch,
                    lane_style.event,
                    style.ruler,
                    style.tick,
                    style.playhead,
                    style.event_selected
                ]
                .contains(&c),
                "unexpected colour {c:?} — the crate must hold no rgba literals"
            );
        }

        // A zero-length axis draws no playhead — there is no frame to stand on.
        let mut empty = Vec::new();
        seated(0).draw(&lanes, &[], &style, 0.0, &mut empty);
        assert!(!empty
            .iter()
            .any(|c| matches!(c, HudCommand::Line { color, .. } if *color == style.playhead)));
    }

    /// A new clip is a new axis: the zoom, the scroll and any live grab go with it.
    #[test]
    fn reset_view_clears_the_window_and_any_grab() {
        let mut t = seated(400);
        t.zoom = 8.0;
        t.scroll_by(100.0);
        let events = [span(0, 10, 20)];
        let bar = t.event_rect(&events[0]);
        t.pointer(
            &PointerSample {
                pressed: true,
                left: true,
                ..sample(bar.pos + bar.size * 0.5)
            },
            &events,
        );
        t.reset_view();
        assert_eq!(t.zoom(), 1.0);
        assert_eq!(t.scroll(), 0.0);
        assert!(t
            .pointer(
                &PointerSample {
                    left: true,
                    ..sample(Vec2::new(t.frame_x(30), bar.pos.y))
                },
                &events
            )
            .edit
            .is_none());
    }
}
