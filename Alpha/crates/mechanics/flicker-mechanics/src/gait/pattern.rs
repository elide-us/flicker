//! THE GAIT TABLE. A gait is a timing: how long a cycle lasts, what fraction of it each foot
//! spends on the ground (the DUTY factor) and where in the cycle each foot lifts (its PHASE).
//! Four foot slots — left fore, right fore, left hind, right hind — cover a quadruped; a biped
//! or a bird uses the hind slots, a hopper both pairs in turn.

use flicker_skeletal::state::{Gait, GaitFamily};

/// The foot slots a pattern's `phase` is indexed by: left fore, right fore, left hind, right hind.
/// Gravity in cm/s², for the Froude number.
pub const GRAVITY_CM: f32 = 981.0;

pub const FOOT_SLOTS: [&str; 4] = ["LF", "RF", "LH", "RH"];

/// The gaits the generator knows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum GaitKind {
    /// Standing: every foot planted, the body breathing.
    Stand,
    /// The lateral-sequence walk: LH · LF · RH · RF, three feet down at a time.
    Walk,
    /// Diagonal pairs in turn.
    Trot,
    /// Lateral pairs in turn (camels, some dogs).
    Pace,
    /// The three-beat canter: LH, then RH + LF, then RF.
    Canter,
    /// The transverse gallop: hinds then fores, a suspension between.
    Gallop,
    /// Small mammals: the fore pair then the hind pair (weasel, squirrel, rat).
    Bound,
    /// Both hinds drive, both fores catch (rabbit, toad).
    Hop,
    /// A low body on legs held out to the sides, the trunk swaying with the walk (lizard,
    /// crocodile, turtle).
    Sprawl,
    /// The walk on a trunk or branch, three feet always holding, the body clinging.
    Climb,
}

/// A gait's timing and shape. Distances are fractions of the LEG LENGTH so one table serves a
/// rat and an aurochs.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GaitPattern {
    pub kind: GaitKind,
    /// The cycle's length at the gait's reference speed (`reference_speed`, in leg lengths per
    /// second); the driver scales it with speed.
    pub period_s: f32,
    pub reference_speed: f32,
    /// The fraction of the cycle each foot spends planted.
    pub duty: f32,
    /// Where in the cycle each foot LIFTS, by [`FOOT_SLOTS`] — the cycle fraction at which its
    /// swing begins.
    pub phase: [f32; 4],
    /// Stride length (one full cycle's travel) in leg lengths, at the reference speed.
    pub stride: f32,
    /// How high a swinging foot lifts, in leg lengths.
    pub step_height: f32,
    /// The trunk's vertical bob, in leg lengths (twice per cycle).
    pub bob: f32,
    /// The trunk's lateral sway, in leg lengths (once per cycle) — the sprawler's undulation.
    pub sway: f32,
    /// How much the body's up follows the support normal (0 = a walker stays level with the
    /// world, 1 = a climber lies on its branch).
    pub cling: f32,
}

/// The table.
pub fn pattern(kind: GaitKind) -> GaitPattern {
    use GaitKind::*;
    let p = |period_s, reference_speed, duty, phase, stride, step_height, bob, sway, cling| {
        GaitPattern {
            kind,
            period_s,
            reference_speed,
            duty,
            phase,
            stride,
            step_height,
            bob,
            sway,
            cling,
        }
    };
    match kind {
        Stand => p(2.0, 0.0, 1.0, [0.0; 4], 0.0, 0.0, 0.01, 0.0, 0.0),
        Walk => p(
            1.2,
            1.0,
            0.65,
            [0.25, 0.75, 0.0, 0.5],
            1.1,
            0.12,
            0.02,
            0.0,
            0.0,
        ),
        Trot => p(
            0.7,
            2.5,
            0.5,
            [0.0, 0.5, 0.5, 0.0],
            1.75,
            0.15,
            0.04,
            0.0,
            0.0,
        ),
        Pace => p(
            0.7,
            2.5,
            0.5,
            [0.0, 0.5, 0.0, 0.5],
            1.75,
            0.15,
            0.04,
            0.03,
            0.0,
        ),
        Canter => p(
            0.55,
            4.0,
            0.42,
            [0.33, 0.66, 0.0, 0.33],
            2.2,
            0.2,
            0.06,
            0.0,
            0.0,
        ),
        Gallop => p(
            0.45,
            6.0,
            0.35,
            [0.5, 0.6, 0.0, 0.1],
            2.7,
            0.25,
            0.08,
            0.0,
            0.0,
        ),
        Bound => p(
            0.5,
            3.0,
            0.4,
            [0.5, 0.5, 0.0, 0.0],
            1.5,
            0.25,
            0.1,
            0.0,
            0.0,
        ),
        Hop => p(
            0.6,
            2.5,
            0.45,
            [0.5, 0.5, 0.0, 0.0],
            1.5,
            0.3,
            0.12,
            0.0,
            0.0,
        ),
        Sprawl => p(
            1.5,
            0.8,
            0.75,
            [0.25, 0.75, 0.0, 0.5],
            1.0,
            0.08,
            0.01,
            0.15,
            0.0,
        ),
        Climb => p(
            1.6,
            0.6,
            0.85,
            [0.25, 0.75, 0.0, 0.5],
            0.8,
            0.15,
            0.0,
            0.0,
            1.0,
        ),
    }
}

/// How a body moves — which gaits it steps through as it speeds up.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LocomotionFamily {
    /// Walk → trot → canter → gallop (the hoofed and pawed quadrupeds).
    Walker,
    /// Walk → trot → gallop with a pace instead of a trot (camels).
    Pacer,
    /// Walk → bound (the small mammals).
    Bounder,
    /// Walk → hop (rabbit, toad).
    Hopper,
    /// Sprawl at every speed (lizard, crocodile, turtle).
    Sprawler,
    /// Climb: the walk with cling (on a trunk or branch).
    Climber,
}

/// Pick a gait for `speed` (cm/s) on legs of `leg_len` (cm) by the FROUDE NUMBER
/// `v² / (g · leg)`: below 0.5 a walk, up to 2.5 a trot (or the family's mid gait), up to 5 a
/// canter, beyond a gallop; a standstill stands.
pub fn select(family: LocomotionFamily, speed: f32, leg_len: f32) -> GaitKind {
    if speed.abs() < 1.0 {
        return GaitKind::Stand;
    }
    let fr = speed * speed / (GRAVITY_CM * leg_len.max(1.0));
    use GaitKind::*;
    match family {
        LocomotionFamily::Sprawler => Sprawl,
        LocomotionFamily::Climber => Climb,
        LocomotionFamily::Hopper => {
            if fr < 0.5 {
                Walk
            } else {
                Hop
            }
        }
        LocomotionFamily::Bounder => {
            if fr < 0.5 {
                Walk
            } else {
                Bound
            }
        }
        LocomotionFamily::Walker | LocomotionFamily::Pacer => {
            if fr < 0.5 {
                Walk
            } else if fr < 2.5 {
                if family == LocomotionFamily::Pacer {
                    Pace
                } else {
                    Trot
                }
            } else if fr < 5.0 {
                Canter
            } else {
                Gallop
            }
        }
    }
}

/// THE PACK'S VOCABULARY → THE DRIVER'S. A pack authors `ClipSource::Generated { gait, family }`
/// in `flicker-skeletal::state` (which cannot see this crate); the one map from those names
/// to the driver's lives HERE, and the gates below hold the two vocabularies in step.
impl From<GaitFamily> for LocomotionFamily {
    fn from(family: GaitFamily) -> Self {
        match family {
            GaitFamily::Walker => LocomotionFamily::Walker,
            GaitFamily::Pacer => LocomotionFamily::Pacer,
            GaitFamily::Bounder => LocomotionFamily::Bounder,
            GaitFamily::Hopper => LocomotionFamily::Hopper,
            GaitFamily::Sprawler => LocomotionFamily::Sprawler,
            GaitFamily::Climber => LocomotionFamily::Climber,
        }
    }
}

/// The driver's gait for a pack gait — `None` for the flight states (`fly`, `glide`, `perch`),
/// which the flap cycle ([`super::flight`]) drives, not the foot planner.
pub fn kind_of(gait: Gait) -> Option<GaitKind> {
    Some(match gait {
        Gait::Stand => GaitKind::Stand,
        Gait::Walk => GaitKind::Walk,
        Gait::Trot => GaitKind::Trot,
        Gait::Pace => GaitKind::Pace,
        Gait::Canter => GaitKind::Canter,
        Gait::Gallop => GaitKind::Gallop,
        Gait::Bound => GaitKind::Bound,
        Gait::Hop => GaitKind::Hop,
        Gait::Sprawl => GaitKind::Sprawl,
        Gait::Climb => GaitKind::Climb,
        Gait::Fly | Gait::Glide | Gait::Perch => return None,
    })
}

/// The speed (cm/s) that MEANS `kind` on legs `leg_len` cm long: a Froude number inside the
/// gait's band in [`select`] — the same number for a rat and an aurochs, which is the point of
/// the scaling. A stand is 0. Gated below against `select` so the bands never drift apart.
pub fn speed_for(kind: GaitKind, leg_len: f32) -> f32 {
    let froude = match kind {
        GaitKind::Stand => return 0.0,
        GaitKind::Walk | GaitKind::Sprawl => 0.25,
        GaitKind::Climb => 0.1,
        GaitKind::Trot | GaitKind::Pace | GaitKind::Hop | GaitKind::Bound => 1.2,
        GaitKind::Canter => 3.5,
        GaitKind::Gallop => 7.0,
    };
    (froude * GRAVITY_CM * leg_len.max(1.0)).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The pack's vocabulary and the driver's stay in step: every family maps, every gait but
    /// the three flight states maps to the driver's kind of the same name, and the speed that
    /// means a gait selects that gait for every family that has it, on a rat's legs and an
    /// aurochs's.
    #[test]
    fn the_pack_vocabulary_maps_onto_the_driver_and_speeds_select_their_gaits() {
        for family in GaitFamily::ALL {
            let _: LocomotionFamily = family.into();
        }
        for gait in Gait::ALL {
            match kind_of(gait) {
                Some(kind) => assert_eq!(
                    format!("{kind:?}").to_lowercase(),
                    gait.name(),
                    "name for name"
                ),
                None => assert!(matches!(gait, Gait::Fly | Gait::Glide | Gait::Perch)),
            }
        }
        let cases: [(LocomotionFamily, &[GaitKind]); 6] = [
            (
                LocomotionFamily::Walker,
                &[
                    GaitKind::Stand,
                    GaitKind::Walk,
                    GaitKind::Trot,
                    GaitKind::Canter,
                    GaitKind::Gallop,
                ],
            ),
            (
                LocomotionFamily::Pacer,
                &[GaitKind::Walk, GaitKind::Pace, GaitKind::Gallop],
            ),
            (
                LocomotionFamily::Bounder,
                &[GaitKind::Walk, GaitKind::Bound],
            ),
            (LocomotionFamily::Hopper, &[GaitKind::Walk, GaitKind::Hop]),
            (LocomotionFamily::Sprawler, &[GaitKind::Sprawl]),
            (LocomotionFamily::Climber, &[GaitKind::Climb]),
        ];
        for (family, kinds) in cases {
            for &kind in kinds {
                for leg in [5.0, 30.0, 90.0, 150.0] {
                    let v = speed_for(kind, leg);
                    assert_eq!(
                        select(family, v, leg),
                        kind,
                        "{family:?} {kind:?} on {leg} cm legs at {v} cm/s"
                    );
                }
            }
        }
    }

    /// The table's invariants: every phase in the cycle, every duty on (0, 1] with the stand
    /// fully planted; the walk is the lateral sequence LH · LF · RH · RF a quarter apart; the
    /// trot pairs diagonals; the gallop has a suspension (duty under a half); the climb clings.
    #[test]
    fn the_gait_table_holds_its_shape() {
        for kind in [
            GaitKind::Stand,
            GaitKind::Walk,
            GaitKind::Trot,
            GaitKind::Pace,
            GaitKind::Canter,
            GaitKind::Gallop,
            GaitKind::Bound,
            GaitKind::Hop,
            GaitKind::Sprawl,
            GaitKind::Climb,
        ] {
            let p = pattern(kind);
            assert_eq!(p.kind, kind);
            assert!(
                p.period_s > 0.0 && p.duty > 0.0 && p.duty <= 1.0,
                "{kind:?}"
            );
            assert!(p.phase.iter().all(|f| (0.0..1.0).contains(f)), "{kind:?}");
        }
        let walk = pattern(GaitKind::Walk);
        let order = |slot: usize| walk.phase[slot];
        // LH first, then LF, then RH, then RF — each a quarter cycle on.
        assert_eq!(
            [order(2), order(0), order(3), order(1)],
            [0.0, 0.25, 0.5, 0.75]
        );
        let trot = pattern(GaitKind::Trot);
        assert_eq!(
            (trot.phase[0], trot.phase[3]),
            (trot.phase[0], trot.phase[0]),
            "LF with RH"
        );
        assert_eq!(trot.phase[1], trot.phase[2], "RF with LH");
        assert!(pattern(GaitKind::Gallop).duty < 0.5, "a suspension");
        assert_eq!(pattern(GaitKind::Stand).duty, 1.0);
        assert_eq!(pattern(GaitKind::Climb).cling, 1.0);
        assert!(pattern(GaitKind::Sprawl).sway > 0.0);
    }

    /// Froude selection: a horse (leg 90 cm) stands still under a centimetre a second, walks at
    /// a metre a second, trots at three, canters at five and gallops at eight; a rabbit hops
    /// where the horse would trot; a lizard sprawls at any speed.
    #[test]
    fn froude_selection_steps_through_the_familys_gaits() {
        use GaitKind::*;
        assert_eq!(select(LocomotionFamily::Walker, 0.0, 90.0), Stand);
        assert_eq!(select(LocomotionFamily::Walker, 100.0, 90.0), Walk);
        assert_eq!(select(LocomotionFamily::Walker, 300.0, 90.0), Trot);
        assert_eq!(select(LocomotionFamily::Pacer, 300.0, 90.0), Pace);
        assert_eq!(select(LocomotionFamily::Walker, 500.0, 90.0), Canter);
        assert_eq!(select(LocomotionFamily::Walker, 800.0, 90.0), Gallop);
        assert_eq!(select(LocomotionFamily::Hopper, 150.0, 12.0), Hop);
        assert_eq!(select(LocomotionFamily::Bounder, 150.0, 10.0), Bound);
        assert_eq!(select(LocomotionFamily::Sprawler, 400.0, 20.0), Sprawl);
        assert_eq!(select(LocomotionFamily::Climber, 30.0, 15.0), Climb);
    }
}
