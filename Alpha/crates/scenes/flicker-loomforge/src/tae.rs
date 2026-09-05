//! The TAE lane VOCABULARY and the authoring BUDGETS — the domain half of the
//! timeline.
//!
//! The strip's geometry (lane placement, frame→pixel mapping, bars, ruler, picking)
//! is not Loomforge's: it is the shared [`flicker_canvas::Timeline`] filler, which the
//! Dungeon Maker's waves and the Game Master's event timelines seat exactly as this
//! bench does. What stays here is what only an animation pack knows — which lanes
//! exist, which event kind belongs on which, and the two budgets an authored window
//! is judged against.
//!
//! All of it is pure and unit-tested; nothing here touches a renderer or a rect.

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

#[cfg(test)]
mod tests {
    use super::*;

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
}
