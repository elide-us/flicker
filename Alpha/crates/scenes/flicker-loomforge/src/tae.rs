//! The **TAE timeline** — the event-track strip under the graph.
//!
//! Scene-drawn for the same reasons as the node canvas: event bars are absolutely
//! positioned along a frame axis, and the walker's component set has no template for a
//! ruler, a playhead, or a bar placed by `{left, width}`. The walker still owns every
//! other piece of chrome on the page.
//!
//! Everything here is a PURE function of the strip rect and the authored events — no
//! renderer, no GPU — so lane placement and frame→pixel mapping are unit-tested directly.

use flicker::render::Vec2;
use flicker_skeletal::state::EventKind;

/// The event lanes, top to bottom. **Window-shaped facts only** — the combat authoring
/// contract's rule is `state-shaped facts → StateDef; window-shaped facts → the timeline`,
/// which is why Root Motion is NOT here (it is `StateDef.root_motion`, a per-state bool; a
/// state property drawn as a lane would be two sources of truth for one fact).
///
/// Ordered defensive-first: the three windows that decide whether a hit lands sit together
/// at the top, then the commitment/announce pair, then the cosmetic channels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lane {
    Hitbox,
    IFrame,
    Parry,
    Cancel,
    HyperArmor,
    Telegraph,
    Sfx,
    Vfx,
    Notify,
}

impl Lane {
    pub const ALL: [Lane; 9] = [
        Lane::Hitbox,
        Lane::IFrame,
        Lane::Parry,
        Lane::Cancel,
        Lane::HyperArmor,
        Lane::Telegraph,
        Lane::Sfx,
        Lane::Vfx,
        Lane::Notify,
    ];

    /// Gutter label.
    pub fn label(self) -> &'static str {
        match self {
            Lane::Hitbox => "HITBOX",
            Lane::IFrame => "I-FRAME",
            Lane::Parry => "PARRY",
            Lane::Cancel => "CANCEL",
            Lane::HyperArmor => "HYPER ARMOR",
            Lane::Telegraph => "TELEGRAPH",
            Lane::Sfx => "SFX",
            Lane::Vfx => "VFX",
            Lane::Notify => "NOTIFY",
        }
    }

    /// Key into `loomforge.tae_lane` — where this lane's four colours live.
    pub fn id(self) -> &'static str {
        match self {
            Lane::Hitbox => "hitbox",
            Lane::IFrame => "iframe",
            Lane::Parry => "parry",
            Lane::Cancel => "cancel",
            Lane::HyperArmor => "hyperarmor",
            Lane::Telegraph => "telegraph",
            Lane::Sfx => "sfx",
            Lane::Vfx => "vfx",
            Lane::Notify => "notify",
        }
    }
}

/// Which lane an authored event belongs on.
///
/// Now **one-to-one for every combat window**: the authoring contract gave `Parry` its own
/// lane (it no longer rides Cancel) and added `HyperArmor`/`Telegraph`, so no window kind is
/// folded onto a neighbour any more. Only the cosmetic kinds still share lanes, where the
/// grouping is the point rather than a compromise.
///
/// **`Parry` is the highest-stakes lane in the strip:** a parry event's `tick` is the
/// server's commit horizon, so where an author puts it sets the game's netcode budget.
///
/// Total by construction — the match is exhaustive and a test pins every kind to a lane.
pub fn lane_of(kind: EventKind) -> Lane {
    match kind {
        EventKind::HitboxActive => Lane::Hitbox,
        EventKind::Iframe => Lane::IFrame,
        EventKind::Parry => Lane::Parry,
        EventKind::CancelWindow => Lane::Cancel,
        EventKind::HyperArmor => Lane::HyperArmor,
        EventKind::Telegraph => Lane::Telegraph,
        EventKind::Sfx | EventKind::Footstep => Lane::Sfx,
        EventKind::WeaponTrail => Lane::Vfx,
        EventKind::Equip => Lane::Notify,
    }
}

// ── budget gauges ─────────────────────────────────────────────────────────────
//
// Two constraints in the pack are invisible today and would surface only as bad feel
// months later, untraceable to the authoring choice that caused them. Both are decided
// HERE, in the editor, so both are made legible here.

/// How a budget reads against its floor. Drives the gauge's colour.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Budget {
    /// Comfortably above the floor.
    Ok,
    /// Above the floor but with little room.
    Tight,
    /// Below the floor — this authored value cannot be delivered.
    Over,
}

impl Budget {
    /// Style key under `loomforge.tae_lane` for this verdict's colour.
    pub fn color_key(self) -> &'static str {
        match self {
            Budget::Ok => "budget_ok",
            Budget::Tight => "budget_tight",
            Budget::Over => "budget_over",
        }
    }
}

/// Ticks → milliseconds at a clip's authored rate. `0` rate falls back to 60 Hz rather
/// than dividing by nothing.
pub fn ticks_to_ms(ticks: u32, rate: u32) -> f32 {
    let hz = if rate == 0 { 60.0 } else { rate as f32 };
    ticks as f32 * 1000.0 / hz
}

/// **The server budget.** A parry event's `tick` — clip start → catch window open — is the
/// window the server has to hear the client's press and still resolve before the animation
/// commits. `min(Parry.tick)` across every parry state IS the game's commit horizon, and
/// this editor is the only place that number is ever chosen.
///
/// Floor is a transcontinental round trip: below it the horizon cannot survive real
/// distance, so the authored window is undeliverable rather than merely tight.
pub fn parry_budget(tick: u32, rate: u32) -> (f32, Budget) {
    let ms = ticks_to_ms(tick, rate);
    let verdict = if ms < COMMIT_HORIZON_FLOOR_MS {
        Budget::Over
    } else if ms < COMMIT_HORIZON_COMFORT_MS {
        Budget::Tight
    } else {
        Budget::Ok
    };
    (ms, verdict)
}

/// **The player budget.** A telegraph window's width is how long the player has to read
/// the attack, recognise *which* answer it admits, and press it. That is a CHOICE
/// reaction, not a simple one — recognising danger is not enough.
///
/// The floor is a per-tier ladder: tighter windows are unlocked by climbing the authoring
/// tech tree, so fairness is a bound the surface enforces rather than a lint a designer can
/// author past. `tier` is `None` until the creature/encounter model exists, in which case
/// the entry rung is used and the caller should say so.
pub fn telegraph_budget(width_ticks: u32, rate: u32, tier: Option<u32>) -> (f32, Budget) {
    let ms = ticks_to_ms(width_ticks, rate);
    let floor = telegraph_floor_ms(tier);
    let verdict = if ms < floor {
        Budget::Over
    } else if ms < floor * TELEGRAPH_COMFORT_FACTOR {
        Budget::Tight
    } else {
        Budget::Ok
    };
    (ms, verdict)
}

/// The narrowest telegraph a given authoring tier may use, in milliseconds.
///
/// Interpolates the ladder between its endpoints, so a bad calibration is fixed by retuning
/// the two endpoints — **without touching a single authored boss**. Ballparks pending
/// playtest, not measurements.
pub fn telegraph_floor_ms(tier: Option<u32>) -> f32 {
    let t = tier.unwrap_or(1).clamp(1, TIER_MAX) as f32;
    let span = (TIER_MAX - 1) as f32;
    let k = if span <= 0.0 { 0.0 } else { (t - 1.0) / span };
    TELEGRAPH_ENTRY_MS + (TELEGRAPH_TOP_MS - TELEGRAPH_ENTRY_MS) * k
}

/// Below this a commit horizon cannot survive a transcontinental round trip.
pub const COMMIT_HORIZON_FLOOR_MS: f32 = 100.0;
/// Below this it works but leaves little slack.
pub const COMMIT_HORIZON_COMFORT_MS: f32 = 133.0;
/// Entry-rung telegraph: choice reaction + commit horizon, generous.
pub const TELEGRAPH_ENTRY_MS: f32 = 480.0;
/// Top-rung telegraph, unlocked at the highest authoring tier.
pub const TELEGRAPH_TOP_MS: f32 = 330.0;
/// Highest authoring tier on the ladder.
pub const TIER_MAX: u32 = 10;
/// Within this multiple of the floor, a telegraph reads as tight rather than comfortable.
const TELEGRAPH_COMFORT_FACTOR: f32 = 1.15;

/// Width of the lane-label gutter (design: 120px).
pub const GUTTER_W: f32 = 120.0;
/// The frame ruler's height, above the lanes.
pub const RULER_H: f32 = 15.0;
/// Vertical gap between lane tracks.
const LANE_GAP: f32 = 2.0;
/// A point event (no end tick) draws this wide instead of as a span (design: 5px).
pub const POINT_W: f32 = 5.0;

/// A rectangle on screen.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub pos: Vec2,
    pub size: Vec2,
}

/// The timeline strip's box on screen.
#[derive(Clone, Copy, Debug)]
pub struct Strip {
    pub pos: Vec2,
    pub size: Vec2,
}

impl Strip {
    /// Left edge of the track area — right of the label gutter.
    pub fn track_x(&self) -> f32 {
        self.pos.x + GUTTER_W
    }

    /// Width of the track area. Never zero, so frame→pixel can't divide by nothing.
    pub fn track_w(&self) -> f32 {
        (self.size.x - GUTTER_W).max(1.0)
    }

    /// Height of one lane track. The seven lanes share whatever is left under the ruler,
    /// so the strip stays correct at any window height rather than only at the mock's.
    pub fn lane_h(&self) -> f32 {
        let n = Lane::ALL.len() as f32;
        ((self.size.y - RULER_H - LANE_GAP * (n + 1.0)) / n).max(1.0)
    }

    /// A lane track's rect.
    pub fn lane_rect(&self, index: usize) -> Rect {
        let h = self.lane_h();
        Rect {
            pos: Vec2::new(
                self.track_x(),
                self.pos.y + RULER_H + LANE_GAP + index as f32 * (h + LANE_GAP),
            ),
            size: Vec2::new(self.track_w(), h),
        }
    }

    /// Where a frame sits along the track. Clamped into the strip, so an event authored
    /// past the end of its clip is still visible at the edge rather than drawn off-screen.
    pub fn frame_x(&self, frame: u32, frames: u32) -> f32 {
        let t = if frames == 0 {
            0.0
        } else {
            (frame as f32 / frames as f32).clamp(0.0, 1.0)
        };
        self.track_x() + t * self.track_w()
    }

    /// An event's bar. `end` absent = a one-shot, drawn as a fixed-width marker; the
    /// point-vs-span distinction comes from the DATA, not from which lane it is on.
    pub fn event_rect(&self, lane: usize, start: u32, end: Option<u32>, frames: u32) -> Rect {
        let track = self.lane_rect(lane);
        let x0 = self.frame_x(start, frames);
        let w = match end {
            Some(e) => (self.frame_x(e.max(start), frames) - x0).max(POINT_W),
            None => POINT_W,
        };
        // Keep the bar inside the track even when it starts at the very last frame.
        let x0 = x0.min(track.pos.x + track.size.x - w);
        Rect {
            pos: Vec2::new(x0, track.pos.y),
            size: Vec2::new(w, track.size.y),
        }
    }
}

/// Ruler tick frames: a round step that keeps the label count readable at any clip
/// length (the design's 40-frame clip yields 0,5,…,40 — nine ticks).
pub fn ruler_ticks(frames: u32) -> Vec<u32> {
    if frames == 0 {
        return vec![0];
    }
    const STEPS: [u32; 9] = [1, 2, 5, 10, 15, 20, 30, 60, 120];
    let step = STEPS
        .iter()
        .copied()
        .find(|s| frames / s <= 10)
        .unwrap_or(frames.max(1));
    (0..=frames / step).map(|i| i * step).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strip() -> Strip {
        Strip {
            pos: Vec2::new(0.0, 900.0),
            size: Vec2::new(1920.0, 180.0),
        }
    }

    /// Every runtime event kind must land on exactly one design lane — an unmapped kind
    /// would silently vanish from the timeline rather than fail loudly.
    #[test]
    fn every_event_kind_maps_onto_a_lane() {
        let kinds = [
            EventKind::Footstep,
            EventKind::HitboxActive,
            EventKind::Iframe,
            EventKind::CancelWindow,
            EventKind::Parry,
            EventKind::HyperArmor,
            EventKind::Telegraph,
            EventKind::Sfx,
            EventKind::Equip,
            EventKind::WeaponTrail,
        ];
        for k in kinds {
            let lane = lane_of(k);
            assert!(
                Lane::ALL.contains(&lane),
                "{k:?} mapped outside the lane set"
            );
        }
        // The authoring contract's rulings, pinned so a later edit has to face them.
        assert_eq!(
            lane_of(EventKind::Parry),
            Lane::Parry,
            "Parry owns its lane — its tick IS the server commit horizon"
        );
        assert_ne!(
            lane_of(EventKind::Parry),
            Lane::Cancel,
            "Parry no longer rides Cancel"
        );
        // Every combat WINDOW kind now has a lane to itself: no folding.
        for k in [
            EventKind::HitboxActive,
            EventKind::Iframe,
            EventKind::Parry,
            EventKind::CancelWindow,
            EventKind::HyperArmor,
            EventKind::Telegraph,
        ] {
            assert_eq!(
                kinds.iter().filter(|o| lane_of(**o) == lane_of(k)).count(),
                1,
                "{k:?} shares its lane with another kind"
            );
        }
        // Lane ids are the ui_theme keys — a collision would cross two lanes' colours.
        let mut ids: Vec<&str> = Lane::ALL.iter().map(|l| l.id()).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), Lane::ALL.len(), "lane ids must be unique");
    }

    /// The contract's worked example: a catch window at tick 8 reads ~133 ms; at tick 4 it
    /// is 66 ms, which does not survive a transcontinental round trip and must read as over
    /// budget rather than merely tight.
    #[test]
    fn parry_budget_flags_an_undeliverable_commit_horizon() {
        let (ms8, v8) = parry_budget(8, 60);
        assert!(
            (ms8 - 133.3).abs() < 0.5,
            "tick 8 at 60 Hz ≈ 133 ms, got {ms8}"
        );
        assert_eq!(v8, Budget::Ok);

        let (ms4, v4) = parry_budget(4, 60);
        assert!(
            (ms4 - 66.7).abs() < 0.5,
            "tick 4 at 60 Hz ≈ 66 ms, got {ms4}"
        );
        assert_eq!(v4, Budget::Over, "66 ms cannot cross an ocean");

        // The verdict follows real time, not tick count: the same tick at half the rate is
        // twice the budget, so a 30 Hz clip is not judged by 60 Hz numbers.
        assert_eq!(parry_budget(4, 30).1, Budget::Ok);
    }

    /// The telegraph floor is a LADDER: the entry rung is generous, the top rung is tight,
    /// and every rung between is bounded by its own floor.
    #[test]
    fn telegraph_floor_tightens_monotonically_with_tier() {
        let entry = telegraph_floor_ms(Some(1));
        let top = telegraph_floor_ms(Some(TIER_MAX));
        assert!((entry - TELEGRAPH_ENTRY_MS).abs() < 0.01);
        assert!((top - TELEGRAPH_TOP_MS).abs() < 0.01);
        assert!(top < entry, "higher tiers unlock tighter windows");
        // Monotone, and out-of-range tiers clamp rather than extrapolate.
        let floors: Vec<f32> = (1..=TIER_MAX)
            .map(|t| telegraph_floor_ms(Some(t)))
            .collect();
        assert!(
            floors.windows(2).all(|w| w[1] <= w[0] + 0.001),
            "ladder never widens"
        );
        assert_eq!(
            telegraph_floor_ms(Some(99)),
            top,
            "tier clamps to the top rung"
        );
        // No tier yet (the creature model is unbuilt) ⇒ the entry rung, the safe default.
        assert_eq!(telegraph_floor_ms(None), entry);
    }

    /// A telegraph narrower than its tier's floor is over budget — the bound the authoring
    /// surface enforces, not a warning it prints.
    #[test]
    fn telegraph_budget_judges_width_against_the_tier_floor() {
        // 12 ticks @60 = 200 ms, well under the 480 ms entry floor.
        assert_eq!(telegraph_budget(12, 60, Some(1)).1, Budget::Over);
        // 30 ticks @60 = 500 ms clears the 480 ms floor, but sits inside its comfort band.
        assert_eq!(telegraph_budget(30, 60, Some(1)).1, Budget::Tight);
        // 36 ticks @60 = 600 ms is comfortably clear.
        assert_eq!(telegraph_budget(36, 60, Some(1)).1, Budget::Ok);
        // The SAME window is legal at the top tier and illegal at the entry rung — the
        // ladder, not a single global threshold, is what decides.
        let (ms, v) = telegraph_budget(21, 60, Some(TIER_MAX));
        assert!((ms - 350.0).abs() < 1.0);
        assert_eq!(
            v,
            Budget::Tight,
            "350 ms clears the 330 ms floor, but only just"
        );
        assert_eq!(
            telegraph_budget(21, 60, Some(1)).1,
            Budget::Over,
            "illegal at tier 1"
        );
        // Clear of the top rung's comfort band entirely.
        assert_eq!(telegraph_budget(30, 60, Some(TIER_MAX)).1, Budget::Ok);
    }

    /// Root motion is a STATE fact and must have NO lane — the contract's
    /// `state-shaped facts → StateDef; window-shaped facts → the timeline` rule.
    #[test]
    fn root_motion_has_no_lane() {
        assert!(
            !Lane::ALL.iter().any(|l| l.id() == "root"),
            "Root Motion left the strip; it lives on StateDef.root_motion"
        );
    }

    /// The seven lanes tile the strip below the ruler, in order, without overlapping.
    #[test]
    fn lanes_stack_in_order_inside_the_strip() {
        let s = strip();
        let rects: Vec<Rect> = (0..Lane::ALL.len()).map(|i| s.lane_rect(i)).collect();
        for w in rects.windows(2) {
            assert!(w[1].pos.y > w[0].pos.y, "lanes descend");
            assert!(
                w[1].pos.y >= w[0].pos.y + w[0].size.y,
                "lanes must not overlap"
            );
        }
        let first = rects[0];
        let last = rects[rects.len() - 1];
        assert!(
            first.pos.y >= s.pos.y + RULER_H,
            "lanes start below the ruler"
        );
        assert!(
            last.pos.y + last.size.y <= s.pos.y + s.size.y + 0.01,
            "the last lane stays inside the strip"
        );
        assert!(
            first.pos.x >= s.pos.x + GUTTER_W,
            "tracks clear the label gutter"
        );
    }

    /// A cramped strip must still produce usable, positive geometry rather than negative
    /// heights that would draw inverted quads.
    #[test]
    fn lane_geometry_survives_a_tiny_strip() {
        let s = Strip {
            pos: Vec2::ZERO,
            size: Vec2::new(40.0, 10.0),
        };
        assert!(s.lane_h() > 0.0);
        assert!(s.track_w() > 0.0);
        let r = s.lane_rect(6);
        assert!(r.size.x > 0.0 && r.size.y > 0.0);
    }

    #[test]
    fn frames_map_across_the_track_and_clamp() {
        let s = strip();
        assert!(
            (s.frame_x(0, 40) - s.track_x()).abs() < 0.01,
            "frame 0 at the left"
        );
        assert!(
            (s.frame_x(40, 40) - (s.track_x() + s.track_w())).abs() < 0.01,
            "the last frame reaches the right"
        );
        assert!(
            s.frame_x(20, 40) > s.frame_x(10, 40),
            "frames advance rightward"
        );
        // A zero-length clip and an over-long event are both survivable.
        assert!(s.frame_x(5, 0).is_finite());
        assert!(
            (s.frame_x(999, 40) - (s.track_x() + s.track_w())).abs() < 0.01,
            "clamped"
        );
    }

    /// A window event spans its frames; a one-shot draws as a marker. Both stay inside
    /// their lane, including one authored on the very last frame.
    #[test]
    fn event_bars_span_windows_and_mark_one_shots() {
        let s = strip();
        let span = s.event_rect(0, 10, Some(20), 40);
        let point = s.event_rect(0, 10, None, 40);
        assert!(
            span.size.x > point.size.x,
            "a window is wider than a one-shot"
        );
        assert_eq!(point.size.x, POINT_W);
        assert_eq!(span.pos.y, s.lane_rect(0).pos.y, "the bar sits in its lane");
        assert_eq!(span.size.y, s.lane_rect(0).size.y);

        // A zero-length window is still visible, not a hairline.
        assert_eq!(s.event_rect(0, 10, Some(10), 40).size.x, POINT_W);
        // An inverted window (end before start) does not produce a negative width.
        assert!(s.event_rect(0, 30, Some(5), 40).size.x > 0.0);

        // An event on the last frame is pulled back inside the track.
        let edge = s.event_rect(0, 40, None, 40);
        let track = s.lane_rect(0);
        assert!(
            edge.pos.x + edge.size.x <= track.pos.x + track.size.x + 0.01,
            "an end-of-clip marker stays inside the track"
        );
    }

    #[test]
    fn ruler_ticks_stay_readable_at_any_clip_length() {
        assert_eq!(ruler_ticks(40), vec![0, 5, 10, 15, 20, 25, 30, 35, 40]);
        for frames in [0, 1, 7, 40, 120, 600, 3600] {
            let t = ruler_ticks(frames);
            assert!(!t.is_empty(), "{frames}: always at least frame 0");
            assert!(t.len() <= 11, "{frames}: {} ticks is unreadable", t.len());
            assert_eq!(t[0], 0);
            assert!(t.windows(2).all(|w| w[1] > w[0]), "{frames}: ticks ascend");
            assert!(
                t.iter().all(|f| *f <= frames.max(1)),
                "{frames}: tick past the clip"
            );
        }
    }
}
