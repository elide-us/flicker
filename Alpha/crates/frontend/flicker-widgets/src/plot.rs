//! `Plot` — the **2D readout filler**: a bounded series of numbers drawn into a
//! `surface` node's reserved rect as a sparkline, a histogram or a filled curve.
//!
//! The shape is the one [`ChatModal`](crate::chat_modal::ChatModal) and
//! `flicker_globe::WorldMap` already established — a component STRUCT the scene
//! HOSTS: the tree authors a plain `surface`, the walker reserves its rect, the
//! scene seats the component on that rect and layers what it draws over the HUD.
//! It is a FILLER and not a walker kind on purpose: a plot is per-SAMPLE geometry,
//! which no arrangement of existing kinds can express (F1BFA408 — decompose before
//! promoting), while everything a walker kind would buy (a Model bind, a style
//! block, hit-testing) it does not need. **The filler never reads the Model.**
//!
//! Three splits keep it honest:
//! - **The scene owns the DATA.** A [`PlotSeries`] is a plain ring the consumer
//!   advances on its own clock (a sim tick, a frame, a job step) — the plot never
//!   samples anything itself, so nothing is recomputed per frame that the consumer
//!   did not ask for (405F7034).
//! - **The scene owns the COLOUR.** Every field of [`PlotStyle`] that is a colour
//!   arrives as a resolved rgba the consumer read out of its own theme tokens.
//!   There is not one rgba literal in this module (the colour-sweep rule): a plot
//!   with an unfilled style draws nothing rather than drawing the wrong ink.
//! - **Rust owns the DRAWING.** Everything below is [`HudCommand`]s — the series as
//!   [`HudCommand::Line`] segments (the primitive landed for exactly this), the bars
//!   and the area fill as `Rect`s. No new pipeline, no pass of its own.
//!
//! # Clip-safe by construction
//! Every command lands strictly INSIDE the seated rect: the plotting band is
//! deflated by half the stroke width so a rotated line quad cannot overhang, and
//! every sample is clamped into the resolved range. The plot therefore emits no
//! [`HudCommand::Clip`] of its own — a clip is a STATE command with no layer, and a
//! filler that pushed one into a host's HUD replay would silently reset the clip
//! the host had set.

use flicker_render::{Rect, Vec2};
use flicker_script::HudCommand;
use std::collections::VecDeque;

use crate::SurfaceSlot;

/// A range narrower than this is FLAT — every sample equal (or a fixed range
/// authored as a point). Padded to [`FLAT_PAD`] either side so the series draws
/// as one rule across the middle instead of dividing by ~zero.
const FLAT_EPS: f32 = 1e-6;
const FLAT_PAD: f32 = 0.5;
/// The bar/curve minimum height (px): a sample sitting exactly on the floor of the
/// range still shows as a mark rather than vanishing.
const MIN_MARK: f32 = 1.0;

/// **The data**: a bounded ring of samples, oldest first, with the range they are
/// plotted against.
///
/// The consumer pushes on its own clock and the ring forgets the oldest sample past
/// `capacity` — so a bench that runs for a million ticks costs `capacity` floats and
/// never grows. Non-finite samples (NaN / ±inf) are KEPT (the ring is a faithful
/// record of what the consumer measured) and simply not drawn — they break the line
/// where the measurement was missing.
#[derive(Clone, Debug)]
pub struct PlotSeries {
    samples: VecDeque<f32>,
    cap: usize,
    /// An authored range, or `None` for auto min/max over the finite samples.
    fixed: Option<(f32, f32)>,
}

impl PlotSeries {
    /// A ring holding at most `capacity` samples (at least one), auto-ranged.
    pub fn new(capacity: usize) -> Self {
        let cap = capacity.max(1);
        Self {
            samples: VecDeque::with_capacity(cap),
            cap,
            fixed: None,
        }
    }

    /// Plot against a FIXED range instead of the samples' own min/max — what a
    /// fraction readout wants (`0.0..1.0`), so the curve does not re-scale itself
    /// every time the world gets slightly warmer.
    pub fn fixed_range(mut self, lo: f32, hi: f32) -> Self {
        self.fixed = Some((lo, hi));
        self
    }

    /// Swap the range at runtime (`None` returns to auto min/max).
    pub fn set_fixed_range(&mut self, range: Option<(f32, f32)>) {
        self.fixed = range;
    }

    /// Record one sample, dropping the oldest once the ring is full.
    pub fn push(&mut self, value: f32) {
        while self.samples.len() >= self.cap {
            self.samples.pop_front();
        }
        self.samples.push_back(value);
    }

    /// Forget every sample (a bench RESET — the era's history goes with the era).
    pub fn clear(&mut self) {
        self.samples.clear();
    }

    /// How many samples the ring currently holds.
    pub fn len(&self) -> usize {
        self.samples.len()
    }

    /// No samples recorded yet.
    pub fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }

    /// The ring's bound — the count it will never exceed.
    pub fn capacity(&self) -> usize {
        self.cap
    }

    /// The samples, oldest first.
    pub fn iter(&self) -> impl Iterator<Item = f32> + '_ {
        self.samples.iter().copied()
    }

    /// The `i`th sample, oldest first.
    pub fn get(&self, i: usize) -> Option<f32> {
        self.samples.get(i).copied()
    }

    /// The newest sample.
    pub fn last(&self) -> Option<f32> {
        self.samples.back().copied()
    }

    /// **The resolved range** the plot maps samples through: the authored
    /// `fixed_range` when there is one, else min/max over the FINITE samples, else
    /// a unit range. Always `hi > lo` — an empty, all-NaN or flat series is padded
    /// so the division below is total and the line lands mid-band.
    pub fn range(&self) -> (f32, f32) {
        if let Some((lo, hi)) = self.fixed {
            return pad_flat(lo, hi);
        }
        let mut lo = f32::INFINITY;
        let mut hi = f32::NEG_INFINITY;
        for v in self.samples.iter().copied().filter(|v| v.is_finite()) {
            lo = lo.min(v);
            hi = hi.max(v);
        }
        if !lo.is_finite() || !hi.is_finite() {
            return (-FLAT_PAD, FLAT_PAD); // nothing measurable yet
        }
        pad_flat(lo, hi)
    }
}

/// Order a range and widen a flat one, so `hi - lo` is always usefully positive.
fn pad_flat(lo: f32, hi: f32) -> (f32, f32) {
    let (lo, hi) = if hi < lo { (hi, lo) } else { (lo, hi) };
    if hi - lo < FLAT_EPS {
        (lo - FLAT_PAD, hi + FLAT_PAD)
    } else {
        (lo, hi)
    }
}

/// How the series is drawn. One data model, three readings of it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PlotKind {
    /// A bare polyline — the inline micro-chart beside a number.
    #[default]
    Sparkline,
    /// A HISTOGRAM: one bar per column, from the range's floor up to the sample.
    Bars,
    /// The polyline with the area beneath it filled — a curve readout.
    Curve,
}

/// The plot's ink and geometry. **Every colour is a resolved rgba the CONSUMER
/// fills from its own theme tokens** — this module never authors one, so a plot
/// cannot smuggle a palette past `ui_theme.json` (the five-line split: Rust draws,
/// JSON places, Lua toggles). Construct it as a struct literal naming the colours
/// you want and `..Default::default()` for the geometry; a colour left at the
/// default is fully transparent and simply does not draw.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PlotStyle {
    /// The series stroke ([`PlotKind::Sparkline`] and [`PlotKind::Curve`]).
    pub line: [f32; 4],
    /// The bars, and the area under a curve.
    pub fill: [f32; 4],
    /// The rule along the bottom of the band.
    pub baseline: [f32; 4],
    /// The interior horizontal rules.
    pub grid: [f32; 4],
    /// Series stroke width in px. Grid and baseline are always hairlines.
    pub width: f32,
    /// Horizontal divisions: `grid_rows - 1` interior rules. `0`/`1` draw none.
    pub grid_rows: u32,
    /// Padding inside the seated rect, px.
    pub inset: f32,
    /// Gap between neighbouring bars, px ([`PlotKind::Bars`] only).
    pub bar_gap: f32,
}

impl Default for PlotStyle {
    fn default() -> Self {
        Self {
            // Unset ink: transparent, never a literal colour (the consumer's job).
            line: [0.0; 4],
            fill: [0.0; 4],
            baseline: [0.0; 4],
            grid: [0.0; 4],
            width: 1.5,
            grid_rows: 2,
            inset: 2.0,
            bar_gap: 1.0,
        }
    }
}

/// **The filler.** Host it beside the [`PlotSeries`] it reads, seat it on the
/// `surface` slot the walker reserved, and layer [`commands`](Self::commands) over
/// the HUD:
///
/// ```ignore
/// self.history.push(sim.temperature());              // on the SIM's clock
/// self.plot.seat(frame.surface(ui::TEMP_PLOT_SLOT));  // the walker reserved it
/// self.hud_commands.extend(self.plot.commands(&self.history));
/// ```
///
/// An unseated plot (the surface is off screen — a dark tab, a closed modal) draws
/// nothing and costs nothing, exactly as a seated-`None` `WorldMap` does.
#[derive(Clone, Debug)]
pub struct Plot {
    kind: PlotKind,
    style: PlotStyle,
    /// The seated image rect and its sub-layer, or `None` while off screen.
    seat: Option<(Rect, f32)>,
}

impl Plot {
    /// A plot of `kind`, inked by `style`.
    pub fn new(kind: PlotKind, style: PlotStyle) -> Self {
        Self {
            kind,
            style,
            seat: None,
        }
    }

    /// Re-ink the plot (a theme swap, a state colour).
    pub fn set_style(&mut self, style: PlotStyle) {
        self.style = style;
    }

    /// Re-read the plot (a Lua toggle between sparkline / bars / curve).
    pub fn set_kind(&mut self, kind: PlotKind) {
        self.kind = kind;
    }

    /// Which reading is drawn.
    pub fn kind(&self) -> PlotKind {
        self.kind
    }

    /// **The hand-off**: take the rect the walker reserved for a `surface` node
    /// (`frame.surface(id)`). A `None` slot — or one with no extent — unseats the
    /// plot, so a gated tab costs nothing.
    pub fn seat(&mut self, slot: Option<&SurfaceSlot>) {
        self.seat = slot.filter(|s| s.w > 0.0 && s.h > 0.0).map(|s| {
            (
                Rect {
                    pos: Vec2::new(s.x, s.y),
                    size: Vec2::new(s.w, s.h),
                },
                s.layer,
            )
        });
    }

    /// Seat on a rect directly — for a host that already owns the geometry (a
    /// catalog card demoing the filler, a headless gate) rather than reserving it
    /// through a `surface` node.
    pub fn seat_rect(&mut self, rect: Rect, layer: f32) {
        self.seat = (rect.size.x > 0.0 && rect.size.y > 0.0).then_some((rect, layer));
    }

    /// The rect currently seated, if any.
    pub fn rect(&self) -> Option<Rect> {
        self.seat.map(|(r, _)| r)
    }

    /// **Draw**: `series` read through this plot's kind and ink, as HUD commands in
    /// painter's order (grid, baseline, fill, stroke) at the seat's layer. Empty
    /// while unseated. Every command lies inside the seated rect.
    pub fn commands(&self, series: &PlotSeries) -> Vec<HudCommand> {
        let Some((rect, layer)) = self.seat else {
            return Vec::new();
        };
        // The plotting BAND: the seat, inset by the authored padding and then by
        // half the stroke, so a line quad's shoulders cannot overhang the seat.
        let hw = (self.style.width * 0.5).max(0.5);
        let pad = self.style.inset.max(0.0) + hw;
        let band = Rect {
            pos: rect.pos + Vec2::splat(pad),
            size: rect.size - Vec2::splat(pad * 2.0),
        };
        if band.size.x <= 0.0 || band.size.y <= 0.0 {
            return Vec::new();
        }
        let (left, top) = (band.pos.x, band.pos.y);
        let (w, h) = (band.size.x, band.size.y);
        let bottom = top + h;

        let mut out = Vec::new();
        // The interior rules, then the floor. Hairlines, so they read as chrome
        // under the series rather than competing with it.
        if self.style.grid[3] > 0.0 {
            for i in 1..self.style.grid_rows {
                let y = top + h * i as f32 / self.style.grid_rows as f32;
                out.push(hairline(left, left + w, y, self.style.grid, layer));
            }
        }
        if self.style.baseline[3] > 0.0 {
            out.push(hairline(left, left + w, bottom, self.style.baseline, layer));
        }

        let n = series.len();
        if n == 0 {
            return out; // an empty series is the band's chrome and nothing else
        }
        let (lo, hi) = series.range();
        let span = hi - lo;
        // A sample's y in the band, clamped: an out-of-range reading rides the edge
        // rather than escaping the seat.
        let y_of = |v: f32| bottom - h * ((v - lo) / span).clamp(0.0, 1.0);

        let cols = column_count(n, w);
        match self.kind {
            PlotKind::Bars => {
                if self.style.fill[3] <= 0.0 {
                    return out;
                }
                let step = w / cols as f32;
                let bw = (step - self.style.bar_gap.max(0.0)).max(MIN_MARK.min(step));
                for c in 0..cols {
                    let Some(v) = finite(series.get(column_sample(n, cols, c))) else {
                        continue; // a missing measurement draws no bar
                    };
                    let y = y_of(v);
                    let bh = (bottom - y).max(MIN_MARK).min(h);
                    out.push(HudCommand::Rect {
                        x: left + step * c as f32 + (step - bw) * 0.5,
                        y: bottom - bh,
                        w: bw,
                        h: bh,
                        color: self.style.fill,
                        layer,
                    });
                }
            }
            PlotKind::Sparkline | PlotKind::Curve => {
                // One point per column, `None` where the measurement was missing —
                // the line BREAKS there rather than inventing a value across it.
                let step = if cols > 1 { w / (cols - 1) as f32 } else { 0.0 };
                let points: Vec<Option<Vec2>> = (0..cols)
                    .map(|c| {
                        let x = if cols > 1 {
                            left + step * c as f32
                        } else {
                            left + w * 0.5
                        };
                        finite(series.get(column_sample(n, cols, c))).map(|v| Vec2::new(x, y_of(v)))
                    })
                    .collect();
                // The area beneath the curve, one column-wide quad per point.
                if self.kind == PlotKind::Curve && self.style.fill[3] > 0.0 {
                    let cw = if cols > 1 { step } else { w };
                    for p in points.iter().flatten() {
                        let x0 = (p.x - cw * 0.5).max(left);
                        let x1 = (p.x + cw * 0.5).min(left + w);
                        let fh = (bottom - p.y).max(MIN_MARK).min(h);
                        out.push(HudCommand::Rect {
                            x: x0,
                            y: bottom - fh,
                            w: x1 - x0,
                            h: fh,
                            color: self.style.fill,
                            layer,
                        });
                    }
                }
                if self.style.line[3] > 0.0 {
                    for pair in points.windows(2) {
                        let (Some(a), Some(b)) = (pair[0], pair[1]) else {
                            continue;
                        };
                        out.push(HudCommand::Line {
                            from: [a.x, a.y],
                            to: [b.x, b.y],
                            width: self.style.width,
                            color: self.style.line,
                            layer,
                        });
                    }
                }
            }
        }
        out
    }
}

/// A 1px rule across the band. It stays inside the seat even at the band's own
/// floor because the band was already deflated by at least half a pixel.
fn hairline(x0: f32, x1: f32, y: f32, color: [f32; 4], layer: f32) -> HudCommand {
    HudCommand::Line {
        from: [x0, y],
        to: [x1, y],
        width: 1.0,
        color,
        layer,
    }
}

/// A sample worth drawing — `None` for a missing (non-finite) measurement.
fn finite(v: Option<f32>) -> Option<f32> {
    v.filter(|v| v.is_finite())
}

/// **How many columns** `n` samples get in a band `w` px wide: at most one per
/// pixel column, never more than there are samples, never fewer than one. This is
/// the whole downsample policy — a 40 000-tick history in a 200 px well costs 200
/// segments, not 40 000.
fn column_count(n: usize, w: f32) -> usize {
    let px = w.floor().max(1.0) as usize;
    n.min(px).max(1)
}

/// **Which sample** feeds column `c` of `cols`, over `n` samples: the columns are
/// spread evenly across the ring so column 0 is the OLDEST sample and the last
/// column is the NEWEST (the reading everyone actually looks at). With one column
/// per sample the mapping is the identity, so a short history is drawn exactly.
fn column_sample(n: usize, cols: usize, c: usize) -> usize {
    if n <= 1 || cols <= 1 {
        return n.saturating_sub(1);
    }
    (c.min(cols - 1) * (n - 1)) / (cols - 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The band a test plot is seated on, and the ink that makes every element
    /// draw (transparent ink is the "unset" state, so a gate that wants commands
    /// must name its colours — exactly as a consumer does).
    fn lit() -> PlotStyle {
        PlotStyle {
            line: [1.0, 0.0, 0.0, 1.0],
            fill: [0.0, 1.0, 0.0, 1.0],
            baseline: [0.0, 0.0, 1.0, 1.0],
            grid: [1.0, 1.0, 1.0, 0.5],
            ..Default::default()
        }
    }

    fn seated(kind: PlotKind) -> Plot {
        let mut p = Plot::new(kind, lit());
        p.seat_rect(
            Rect {
                pos: Vec2::new(100.0, 50.0),
                size: Vec2::new(200.0, 40.0),
            },
            0.5,
        );
        p
    }

    fn series(values: &[f32]) -> PlotSeries {
        let mut s = PlotSeries::new(values.len().max(1));
        for v in values {
            s.push(*v);
        }
        s
    }

    fn lines(cmds: &[HudCommand]) -> usize {
        cmds.iter()
            .filter(|c| matches!(c, HudCommand::Line { .. }))
            .count()
    }

    fn rects(cmds: &[HudCommand]) -> usize {
        cmds.iter()
            .filter(|c| matches!(c, HudCommand::Rect { .. }))
            .count()
    }

    /// The chrome every kind draws before a single sample: `grid_rows - 1`
    /// interior rules plus the floor.
    fn chrome(style: &PlotStyle) -> usize {
        (style.grid_rows.saturating_sub(1)) as usize + 1
    }

    #[test]
    fn the_ring_forgets_the_oldest_sample_past_its_capacity() {
        let mut s = PlotSeries::new(3);
        assert!(s.is_empty());
        for v in [1.0, 2.0, 3.0, 4.0, 5.0] {
            s.push(v);
        }
        assert_eq!(s.len(), 3, "the ring is BOUNDED, not a log");
        assert_eq!(s.capacity(), 3);
        assert_eq!(s.iter().collect::<Vec<_>>(), vec![3.0, 4.0, 5.0]);
        assert_eq!(s.last(), Some(5.0), "the newest sample is the last");
        s.clear();
        assert!(s.is_empty(), "a reset takes the history with it");
        // A zero capacity would be a ring that silently swallows every push.
        assert_eq!(PlotSeries::new(0).capacity(), 1);
    }

    #[test]
    fn columns_spread_the_ring_evenly_and_end_on_the_newest_sample() {
        // One column per sample is the identity — a short history draws exactly.
        assert_eq!(column_count(8, 200.0), 8);
        for c in 0..8 {
            assert_eq!(column_sample(8, 8, c), c);
        }
        // More samples than pixels: one segment per pixel column, no more.
        assert_eq!(column_count(40_000, 200.0), 200);
        assert_eq!(column_sample(40_000, 200, 0), 0, "column 0 is the oldest");
        assert_eq!(
            column_sample(40_000, 200, 199),
            39_999,
            "the last column is the NEWEST sample — the reading being watched"
        );
        // Monotone, so the curve never doubles back on itself.
        let mut last = 0;
        for c in 0..200 {
            let i = column_sample(40_000, 200, c);
            assert!(i >= last, "column {c} walked backwards");
            last = i;
        }
        // Degenerate bands and rings still answer.
        assert_eq!(column_count(0, 0.0), 1);
        assert_eq!(column_sample(1, 1, 0), 0);
    }

    #[test]
    fn the_range_is_auto_by_default_and_fixed_when_authored() {
        let s = series(&[2.0, -1.0, 5.0, 0.0]);
        assert_eq!(s.range(), (-1.0, 5.0), "auto range is the samples' own span");
        let s = series(&[2.0, -1.0, 5.0]).fixed_range(0.0, 10.0);
        assert_eq!(s.range(), (0.0, 10.0), "an authored range wins");
        let mut s = s;
        s.set_fixed_range(None);
        assert_eq!(s.range(), (-1.0, 5.0), "and can be handed back");
        // A backwards authored range is ORDERED rather than dividing by a negative.
        assert_eq!(PlotSeries::new(4).fixed_range(9.0, 1.0).range(), (1.0, 9.0));
    }

    #[test]
    fn empty_flat_and_all_nan_series_draw_the_chrome_and_nothing_else() {
        let style = lit();
        for kind in [PlotKind::Sparkline, PlotKind::Bars, PlotKind::Curve] {
            let p = seated(kind);
            // EMPTY: the band's chrome, no series marks at all.
            let empty = p.commands(&PlotSeries::new(16));
            assert_eq!(lines(&empty) + rects(&empty), chrome(&style), "{kind:?} empty");
            // ALL NaN: a full ring of missing measurements draws exactly the same.
            let nan = series(&[f32::NAN; 6]);
            let cmds = p.commands(&nan);
            assert_eq!(
                lines(&cmds) + rects(&cmds),
                chrome(&style),
                "{kind:?} over an all-NaN ring"
            );
            // FLAT: a constant series is a rule across the middle, not a NaN storm.
            let flat = series(&[0.25; 5]);
            let cmds = p.commands(&flat);
            for c in &cmds {
                if let HudCommand::Line { from, to, .. } = c {
                    assert!(
                        from[1].is_finite() && to[1].is_finite(),
                        "{kind:?} flat series produced a non-finite point"
                    );
                }
            }
            let (lo, hi) = flat.range();
            assert!(hi > lo, "a flat series still resolves to a usable range");
        }
    }

    #[test]
    fn a_gap_in_the_measurements_breaks_the_line_instead_of_bridging_it() {
        let p = seated(PlotKind::Sparkline);
        let whole = p.commands(&series(&[0.0, 1.0, 2.0, 3.0, 4.0]));
        let holed = p.commands(&series(&[0.0, 1.0, f32::NAN, 3.0, 4.0]));
        assert_eq!(lines(&whole), chrome(&lit()) + 4, "4 segments over 5 samples");
        assert_eq!(
            lines(&holed),
            chrome(&lit()) + 2,
            "the two segments touching the missing sample are not drawn"
        );
    }

    #[test]
    fn bars_draw_a_quad_per_column_and_a_sparkline_draws_a_segment_between_them() {
        let samples = [0.0, 0.5, 1.0, 0.25, 0.75, 0.1];
        let n = samples.len();
        let chrome = chrome(&lit());

        let bars = seated(PlotKind::Bars).commands(&series(&samples));
        assert_eq!(rects(&bars), n, "one bar per column — a histogram");
        assert_eq!(lines(&bars), chrome, "bars are quads; the rules are the chrome");

        let spark = seated(PlotKind::Sparkline).commands(&series(&samples));
        assert_eq!(rects(&spark), 0, "a sparkline is stroke only");
        assert_eq!(lines(&spark), chrome + (n - 1), "n samples, n-1 segments");

        // A curve is the sparkline PLUS its area — same stroke, quads beneath.
        let curve = seated(PlotKind::Curve).commands(&series(&samples));
        assert_eq!(lines(&curve), chrome + (n - 1));
        assert_eq!(rects(&curve), n, "the area is one column quad per point");
    }

    #[test]
    fn an_unseated_plot_draws_nothing_and_a_seat_with_no_extent_unseats_it() {
        let mut p = Plot::new(PlotKind::Curve, lit());
        assert!(p.rect().is_none());
        assert!(p.commands(&series(&[1.0, 2.0])).is_empty(), "no seat, no cost");
        p.seat(None);
        assert!(p.commands(&series(&[1.0, 2.0])).is_empty());
        // A gated tab reserves a zero-extent slot; that is not a seat.
        p.seat_rect(
            Rect {
                pos: Vec2::ZERO,
                size: Vec2::ZERO,
            },
            0.0,
        );
        assert!(p.rect().is_none());
        // And a seat smaller than its own padding draws nothing rather than
        // inverting the band.
        p.seat_rect(
            Rect {
                pos: Vec2::new(10.0, 10.0),
                size: Vec2::new(2.0, 2.0),
            },
            0.0,
        );
        assert!(p.commands(&series(&[1.0, 2.0])).is_empty());
    }

    #[test]
    fn every_command_lands_inside_the_seated_rect() {
        // Wild samples, a fixed range they blow through, and a fat stroke: the
        // plot is clip-safe by CONSTRUCTION, so it never needs a Clip of its own.
        let mut s = PlotSeries::new(64).fixed_range(0.0, 1.0);
        for i in 0..64 {
            s.push(if i % 7 == 0 { -50.0 } else { i as f32 });
        }
        let style = PlotStyle {
            width: 4.0,
            ..lit()
        };
        for kind in [PlotKind::Sparkline, PlotKind::Bars, PlotKind::Curve] {
            let mut p = Plot::new(kind, style);
            let rect = Rect {
                pos: Vec2::new(100.0, 50.0),
                size: Vec2::new(200.0, 40.0),
            };
            p.seat_rect(rect, 0.5);
            let (x0, y0) = (rect.pos.x, rect.pos.y);
            let (x1, y1) = (x0 + rect.size.x, y0 + rect.size.y);
            let cmds = p.commands(&s);
            assert!(!cmds.is_empty(), "{kind:?} drew something");
            for c in &cmds {
                match c {
                    HudCommand::Line {
                        from, to, width, ..
                    } => {
                        let hw = width * 0.5;
                        for pt in [from, to] {
                            assert!(
                                pt[0] >= x0 - 0.01
                                    && pt[0] <= x1 + 0.01
                                    && pt[1] - hw >= y0 - 0.01
                                    && pt[1] + hw <= y1 + 0.01,
                                "{kind:?} line escaped the seat at {pt:?}"
                            );
                        }
                    }
                    HudCommand::Rect { x, y, w, h, .. } => assert!(
                        *x >= x0 - 0.01
                            && *y >= y0 - 0.01
                            && x + w <= x1 + 0.01
                            && y + h <= y1 + 0.01,
                        "{kind:?} quad escaped the seat at {x},{y} {w}×{h}"
                    ),
                    other => panic!("{kind:?} emitted an unexpected command: {other:?}"),
                }
            }
            assert!(
                !cmds
                    .iter()
                    .any(|c| matches!(c, HudCommand::Clip { .. })),
                "a filler never resets its host's clip"
            );
        }
    }

    #[test]
    fn unfilled_ink_draws_nothing_rather_than_the_wrong_colour() {
        // The colour-sweep rule: the module owns no palette, so a style the
        // consumer never filled is INVISIBLE — never a stand-in colour.
        let mut p = Plot::new(PlotKind::Sparkline, PlotStyle::default());
        p.seat_rect(
            Rect {
                pos: Vec2::ZERO,
                size: Vec2::new(120.0, 30.0),
            },
            0.0,
        );
        assert!(p.commands(&series(&[1.0, 2.0, 3.0])).is_empty());
        // The kind and ink are both re-settable at runtime (a Lua toggle).
        p.set_style(lit());
        p.set_kind(PlotKind::Bars);
        assert_eq!(p.kind(), PlotKind::Bars);
        assert!(!p.commands(&series(&[1.0, 2.0, 3.0])).is_empty());
    }

    #[test]
    fn the_seat_carries_the_slots_layer_so_the_plot_sorts_with_its_surface() {
        let slot = SurfaceSlot {
            id: "probe".into(),
            source: String::new(),
            x: 4.0,
            y: 8.0,
            w: 160.0,
            h: 32.0,
            layer: 2.5,
            rate: Default::default(),
            tint: [1.0; 4],
            layout: flicker_render::ViewportLayout::Single,
        };
        let mut p = Plot::new(PlotKind::Sparkline, lit());
        p.seat(Some(&slot));
        let r = p.rect().expect("the slot seats the plot");
        assert_eq!(r.pos, Vec2::new(4.0, 8.0));
        assert_eq!(r.size, Vec2::new(160.0, 32.0));
        for c in p.commands(&series(&[0.0, 1.0, 0.5])) {
            let layer = match c {
                HudCommand::Line { layer, .. } | HudCommand::Rect { layer, .. } => layer,
                other => panic!("unexpected command: {other:?}"),
            };
            assert_eq!(layer, 2.5, "every mark rides the slot's own band");
        }
    }
}
