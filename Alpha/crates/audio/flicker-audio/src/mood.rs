//! The progression engine: four dials and a seed in, [`Score`] rows out.
//!
//! The emotional trajectory is DATA — rows of dials at bar positions. The
//! generator renders it: per four-bar phrase it picks the mode from brightness,
//! stretches every progression template over the phrase's chord slots, scores
//! each template's tension curve against the trajectory and takes the best
//! (a seeded pick among near-ties), voices the chords with the least motion,
//! and lays bass, pad, melody and colour onto the rhythm pattern density asks
//! for. A lookup over rows, never a constraint solver; the same seed and dials
//! always give the same rows.

use crate::arrangement::{compose, Form, PlacedSection};
use crate::melody::MelodyPlan;
use crate::patch::{program_name, KIT_CHANNEL};
use crate::score::{Event, Note, Score};
use crate::texture::{PROGRAM_BREEZE, PROGRAM_CRICKETS, PROGRAM_FROGS};
use crate::theory::{diatonic_chord, roman, Chord, Function, Mode, Template};
use crate::time::Pos;

/// The abstract control surface, each 0..1. No theory words.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Dials {
    pub tension: f32,
    pub brightness: f32,
    pub density: f32,
    pub pace: f32,
}

impl Dials {
    fn clamped(self) -> Dials {
        Dials {
            tension: self.tension.clamp(0.0, 1.0),
            brightness: self.brightness.clamp(0.0, 1.0),
            density: self.density.clamp(0.0, 1.0),
            pace: self.pace.clamp(0.0, 1.0),
        }
    }

    fn lerp(a: Dials, b: Dials, t: f32) -> Dials {
        let l = |x: f32, y: f32| x + (y - x) * t;
        Dials {
            tension: l(a.tension, b.tension),
            brightness: l(a.brightness, b.brightness),
            density: l(a.density, b.density),
            pace: l(a.pace, b.pace),
        }
    }
}

/// The trajectory: dial rows at positions, linearly interpolated between them,
/// held flat before the first and after the last.
#[derive(Clone, Debug, PartialEq)]
pub struct Trajectory {
    rows: Vec<(Pos, Dials)>,
}

impl Trajectory {
    pub fn flat(dials: Dials) -> Trajectory {
        Trajectory {
            rows: vec![(Pos::ZERO, dials.clamped())],
        }
    }

    pub fn new(mut rows: Vec<(Pos, Dials)>) -> Trajectory {
        assert!(!rows.is_empty(), "a trajectory needs at least one row");
        rows.sort_by_key(|(at, _)| *at);
        for (_, d) in &mut rows {
            *d = d.clamped();
        }
        Trajectory { rows }
    }

    pub fn rows(&self) -> &[(Pos, Dials)] {
        &self.rows
    }

    pub fn at(&self, pos: Pos) -> Dials {
        let (first, last) = (&self.rows[0], &self.rows[self.rows.len() - 1]);
        if pos <= first.0 {
            return first.1;
        }
        if pos >= last.0 {
            return last.1;
        }
        let i = self
            .rows
            .iter()
            .rposition(|(at, _)| *at <= pos)
            .unwrap_or(0);
        let (a, b) = (&self.rows[i], &self.rows[i + 1]);
        let span = (b.0 - a.0).to_f64();
        let t = if span > 0.0 {
            ((pos - a.0).to_f64() / span) as f32
        } else {
            0.0
        };
        Dials::lerp(a.1, b.1, t)
    }
}

/// Preset shapes so movement can be heard from a few flags.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Arc {
    /// The dials as given, for the whole piece.
    Flat,
    /// From quiet and low tension up to the dials.
    Rise,
    /// From the dials down to quiet and low tension.
    Fall,
    /// Up to the dials by the middle, back down by the end.
    Swell,
}

impl Arc {
    pub fn parse(s: &str) -> Option<Arc> {
        Some(match s {
            "flat" => Arc::Flat,
            "rise" => Arc::Rise,
            "fall" => Arc::Fall,
            "swell" => Arc::Swell,
            _ => return None,
        })
    }

    /// Build the trajectory rows for `bars` of music at these dials.
    pub fn trajectory(self, dials: Dials, bars: u32) -> Trajectory {
        let low = Dials {
            tension: 0.1,
            density: 0.15,
            ..dials
        };
        let end = Pos::new(bars.max(1) as i64, 1);
        let mid = Pos::new(bars.max(1) as i64, 2);
        match self {
            Arc::Flat => Trajectory::flat(dials),
            Arc::Rise => Trajectory::new(vec![(Pos::ZERO, low), (end, dials)]),
            Arc::Fall => Trajectory::new(vec![(Pos::ZERO, dials), (end, low)]),
            Arc::Swell => Trajectory::new(vec![(Pos::ZERO, low), (mid, dials), (end, low)]),
        }
    }
}

/// Who plays what — the starter role rows. Channel index = position.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    Pad,
    Bass,
    Melody,
    Colour,
    /// A single held tone, usually the tonic.
    Drone,
    /// The progression doubled an octave up on an ensemble.
    Strings,
    /// The mid-range melody on sparse cells.
    Horn,
    /// Struck roots on the downbeats.
    Timpani,
    /// Wind: a texture, one note per phrase.
    Breeze,
    /// Crickets: a texture, one note per individual.
    Crickets,
    /// Frogs: a texture, one note per individual.
    Frogs,
    /// Orchestral march percussion on the kit channel.
    March,
}

pub const ROLES: [Role; 12] = [
    Role::Pad,
    Role::Bass,
    Role::Melody,
    Role::Colour,
    Role::Drone,
    Role::Strings,
    Role::Horn,
    Role::Timpani,
    Role::Breeze,
    Role::Crickets,
    Role::Frogs,
    Role::March,
];

impl Role {
    /// The channel a role plays on; the march is the kit channel.
    pub fn channel(self) -> u8 {
        match self {
            Role::Pad => 0,
            Role::Bass => 1,
            Role::Melody => 2,
            Role::Colour => 3,
            Role::Drone => 4,
            Role::Strings => 5,
            Role::Horn => 6,
            Role::Timpani => 7,
            Role::Breeze => 8,
            Role::March => KIT_CHANNEL,
            Role::Crickets => 10,
            Role::Frogs => 11,
        }
    }

    /// General MIDI program for the role. Melody picks by brightness.
    pub fn program(self, brightness: f32) -> u8 {
        match self {
            Role::Pad => 89,
            Role::Bass => 33,
            Role::Melody => {
                if brightness >= 0.5 {
                    73
                } else {
                    71
                }
            }
            Role::Colour => 10,
            Role::Drone => 92,
            Role::Strings => 48,
            Role::Horn => 60,
            Role::Timpani => 47,
            Role::Breeze => PROGRAM_BREEZE,
            Role::Crickets => PROGRAM_CRICKETS,
            Role::Frogs => PROGRAM_FROGS,
            // The kit channel ignores programs.
            Role::March => 0,
        }
    }

    pub fn volume(self) -> f32 {
        match self {
            Role::Pad => 0.65,
            Role::Bass => 0.9,
            Role::Melody => 0.8,
            Role::Colour => 0.55,
            Role::Drone => 0.55,
            Role::Strings => 0.45,
            Role::Horn => 0.75,
            Role::Timpani => 0.7,
            Role::Breeze => 0.4,
            Role::Crickets => 0.3,
            Role::Frogs => 0.35,
            Role::March => 0.85,
        }
    }

    /// Send into the distance bus: how far away the layer sits.
    pub fn reverb(self) -> f32 {
        match self {
            Role::Crickets => 0.85,
            Role::Frogs => 0.8,
            Role::Breeze => 0.6,
            Role::Drone => 0.4,
            Role::Colour => 0.5,
            _ => 0.0,
        }
    }

    pub fn pan(self) -> f32 {
        match self {
            Role::Pad => 0.0,
            Role::Bass => 0.0,
            Role::Melody => 0.2,
            Role::Colour => -0.3,
            Role::Drone => 0.0,
            Role::Strings => 0.25,
            Role::Horn => -0.15,
            Role::Timpani => -0.1,
            Role::Breeze => 0.0,
            Role::Crickets => 0.35,
            Role::Frogs => -0.4,
            Role::March => 0.0,
        }
    }
}

/// A rhythm pattern over one chord slot: onsets as fractions of the slot;
/// each onset lasts one `den`-th of the slot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pattern {
    pub name: &'static str,
    pub onsets: &'static [(i64, i64)],
}

/// Density tiers, sparse to busy. The last row is the triplet feel.
pub const PATTERNS: [Pattern; 4] = [
    Pattern {
        name: "hold",
        onsets: &[(0, 1)],
    },
    Pattern {
        name: "halves",
        onsets: &[(0, 2), (1, 2)],
    },
    Pattern {
        name: "quarters",
        onsets: &[(0, 4), (1, 4), (2, 4), (3, 4)],
    },
    Pattern {
        name: "triplets",
        onsets: &[(0, 3), (1, 3), (2, 3)],
    },
];

pub fn pattern_for(density: f32) -> &'static Pattern {
    let tier = if density < 0.3 {
        0
    } else if density < 0.55 {
        1
    } else if density < 0.8 {
        2
    } else {
        3
    };
    &PATTERNS[tier]
}

/// Quarter-note beats per minute for a pace dial.
pub fn bpm_for(pace: f32) -> f64 {
    60.0 + 100.0 * pace.clamp(0.0, 1.0) as f64
}

#[allow(dead_code)]
fn chords_per_bar(pace: f32) -> i64 {
    if pace >= 0.5 {
        2
    } else {
        1
    }
}

pub(crate) fn tones_for(tension: f32, density: f32) -> usize {
    if tension >= 0.6 || density >= 0.7 {
        4
    } else {
        3
    }
}

/// Melody and colour join above these densities.
pub const MELODY_DENSITY: f32 = 0.5;
pub const COLOUR_DENSITY: f32 = 0.8;

/// How far the target may sit above a tonic-function chord's tension before
/// the renderer lifts that chord to the dominant.
pub const LIFT_GAP: f32 = 0.3;
/// The lift only acts on genuinely high targets; below this the templates'
/// own tension curves are the whole answer.
pub const LIFT_FLOOR: f32 = 0.6;

/// The degree a template puts in slot `j` of `slots`: loop the template when
/// it divides the phrase, keep its ending when the phrase is shorter than it,
/// stretch it otherwise.
pub fn degree_for_slot(template: &Template, j: usize, slots: usize) -> u8 {
    let len = template.degrees.len();
    if slots.is_multiple_of(len) {
        template.degrees[j % len]
    } else if slots < len {
        // Too few slots: keep the template's ENDING, where it resolves.
        template.degrees[len - slots + j]
    } else {
        template.degrees[j * len / slots]
    }
}

/// The chords a template yields for these slot dials, with the tension lift
/// applied: a tonic-function chord sitting more than [`LIFT_GAP`] under its
/// target becomes the dominant — never on a phrase's last slot, which is where
/// a progression resolves.
pub(crate) fn realize(template: &Template, mode: Mode, dials: &[Dials]) -> Vec<(Chord, bool)> {
    let slots = dials.len();
    (0..slots)
        .map(|j| {
            let d = dials[j];
            let tones = tones_for(d.tension, d.density);
            let chord = diatonic_chord(mode, degree_for_slot(template, j, slots) as usize, tones);
            let final_slot = j + 1 == slots;
            if chord.function == Function::Tonic
                && d.tension >= LIFT_FLOOR
                && d.tension - chord.tension > LIFT_GAP
                && !final_slot
            {
                (diatonic_chord(mode, 4, tones), true)
            } else {
                (chord, false)
            }
        })
        .collect()
}

/// One chord as placed in the piece.
#[derive(Clone, Debug, PartialEq)]
pub struct PlacedChord {
    pub at: Pos,
    pub dur: Pos,
    pub chord: Chord,
    pub voicing: Vec<u8>,
    /// Lifted to the dominant to meet the tension target.
    pub lifted: bool,
    /// The dials in force at this slot.
    pub dials: Dials,
}

/// A four-bar (or shorter, final) phrase and what was chosen for it.
#[derive(Clone, Debug, PartialEq)]
pub struct Phrase {
    pub start_bar: u32,
    pub bars: u32,
    pub mode: Mode,
    pub template: &'static str,
    pub pattern: &'static str,
    pub chords: Vec<PlacedChord>,
    pub target_tension: f32,
    pub achieved_tension: f32,
    /// How many chords the tension lift replaced.
    pub lifts: usize,
    /// Chord slots per melodic unit (1 at one chord per bar, 2 at two).
    pub unit_slots: usize,
    /// What the melody layer did here; `None` when density kept it silent.
    pub melody: Option<MelodyPlan>,
}

impl Phrase {
    /// "I  V*  vi  IV" for readouts; `*` marks a lifted chord.
    pub fn numerals(&self) -> String {
        self.chords
            .iter()
            .map(|c| format!("{}{}", roman(&c.chord), if c.lifted { "*" } else { "" }))
            .collect::<Vec<_>>()
            .join("  ")
    }
}

/// The generated piece: the rows plus the chart of what was decided.
#[derive(Clone, Debug, PartialEq)]
pub struct Piece {
    pub score: Score,
    pub phrases: Vec<Phrase>,
    /// The form's sections as placed.
    pub sections: Vec<PlacedSection>,
    pub tonic: u8,
    pub seed: u64,
}

/// The dial-driven loop: `bars` of music from the trajectory, every layer by
/// density, a fresh template each phrase. The one-section form.
pub fn generate(seed: u64, trajectory: &Trajectory, bars: u32, tonic: u8) -> Piece {
    compose(&Form::looped(bars), seed, trajectory, tonic)
}

pub(crate) fn push_note(
    score: &mut Score,
    role: Role,
    at: Pos,
    dur: Pos,
    pitch: u8,
    velocity: f32,
) {
    score.events.push(Event::Note(Note {
        at,
        dur,
        channel: role.channel(),
        pitch: pitch.min(127),
        velocity: velocity.clamp(0.0, 1.0),
    }));
}

/// Place the chord's tones as close as possible to the previous voicing and
/// the register centre: every rotation, three octaves, least total motion.
pub(crate) fn voice(chord: &Chord, tonic: u8, center: i32, prev: Option<&[u8]>) -> Vec<u8> {
    let n = chord.tones.len();
    let pcs: Vec<i32> = chord
        .tones
        .iter()
        .map(|t| (tonic as i32 + *t as i32).rem_euclid(12))
        .collect();
    let mut best: Option<(f32, Vec<u8>)> = None;
    for rot in 0..n {
        for base in [center - 12, center, center + 12] {
            let floor = base - 6;
            let mut v: Vec<i32> = vec![floor + (pcs[rot] - floor).rem_euclid(12)];
            for k in 1..n {
                let pc = pcs[(rot + k) % n];
                let above = v[k - 1] + 1;
                v.push(above + (pc - above).rem_euclid(12));
            }
            let mean = v.iter().sum::<i32>() as f32 / n as f32;
            let motion = prev
                .map(|pv| {
                    let moved: f32 = v
                        .iter()
                        .zip(pv.iter())
                        .map(|(a, b)| (*a as f32 - *b as f32).abs())
                        .sum();
                    moved + (n as f32 - pv.len() as f32).abs() * 2.0
                })
                .unwrap_or(0.0);
            let cost = motion + 0.35 * (mean - center as f32).abs();
            if best.as_ref().is_none_or(|(c, _)| cost < *c) {
                best = Some((cost, v.iter().map(|p| (*p).clamp(0, 127) as u8).collect()));
            }
        }
    }
    best.expect("a chord has at least three tones").1
}

/// Roles and their programs for a brightness, for readouts.
pub fn role_chart(brightness: f32) -> String {
    ROLES
        .iter()
        .map(|r| format!("{:?}={}", r, program_name(r.program(brightness))).to_lowercase())
        .collect::<Vec<_>>()
        .join("  ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theory::{LIGHTNESS, TEMPLATES};

    fn dials(t: f32, b: f32, d: f32, p: f32) -> Dials {
        Dials {
            tension: t,
            brightness: b,
            density: d,
            pace: p,
        }
    }

    #[test]
    fn trajectories_interpolate_and_hold_at_the_ends() {
        let tr = Trajectory::new(vec![
            (Pos::new(2, 1), dials(0.2, 0.5, 0.5, 0.5)),
            (Pos::new(6, 1), dials(1.0, 0.5, 0.5, 0.5)),
        ]);
        assert_eq!(tr.at(Pos::ZERO).tension, 0.2);
        assert!((tr.at(Pos::new(4, 1)).tension - 0.6).abs() < 1e-6);
        assert_eq!(tr.at(Pos::new(9, 1)).tension, 1.0);
        let rise = Arc::Rise.trajectory(dials(0.9, 0.5, 0.9, 0.5), 8);
        assert_eq!(rise.at(Pos::ZERO).tension, 0.1);
        assert_eq!(rise.at(Pos::new(8, 1)).tension, 0.9);
        let swell = Arc::Swell.trajectory(dials(0.9, 0.5, 0.9, 0.5), 8);
        assert_eq!(swell.at(Pos::new(4, 1)).tension, 0.9);
        assert_eq!(swell.at(Pos::new(8, 1)).tension, 0.1);
        assert_eq!(Arc::parse("swell"), Some(Arc::Swell));
        assert_eq!(Arc::parse("nope"), None);
    }

    #[test]
    fn same_seed_and_dials_give_identical_rows_and_another_seed_does_not() {
        let tr = Trajectory::flat(dials(0.5, 0.6, 0.6, 0.5));
        let a = generate(7, &tr, 8, 0);
        let b = generate(7, &tr, 8, 0);
        assert_eq!(a, b);
        let c = generate(8, &tr, 8, 0);
        assert_ne!(a.score, c.score);
        assert_eq!(a.score.end, Pos::new(8, 1));
        assert_eq!(a.phrases.len(), 2);
        assert_eq!(a.phrases[1].start_bar, 4);
    }

    #[test]
    fn every_pitch_is_in_the_phrase_mode_and_voicings_move_little() {
        for (seed, b) in [(1u64, 0.0f32), (2, 0.3), (3, 0.5), (4, 0.8), (5, 1.0)] {
            let tr = Trajectory::flat(dials(0.4, b, 0.9, 0.6));
            let piece = generate(seed, &tr, 8, 7);
            let mode = Mode::for_brightness(b);
            assert!(LIGHTNESS.contains(&mode));
            for n in piece.score.notes() {
                let pc = (n.pitch as i32 - piece.tonic as i32).rem_euclid(12) as u8;
                assert!(
                    mode.contains(pc),
                    "seed {seed} pitch {} outside {mode:?}",
                    n.pitch
                );
            }
            for phrase in &piece.phrases {
                assert_eq!(phrase.mode, mode);
                for pair in phrase.chords.windows(2) {
                    let (a, b) = (&pair[0].voicing, &pair[1].voicing);
                    let moved: i32 = a
                        .iter()
                        .zip(b)
                        .map(|(x, y)| (*x as i32 - *y as i32).abs())
                        .sum();
                    assert!(
                        moved <= 4 * a.len() as i32,
                        "voicing leapt {moved}: {a:?} → {b:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn density_adds_roles_and_the_top_tier_is_triplets() {
        let sparse = generate(1, &Trajectory::flat(dials(0.2, 0.7, 0.1, 0.3)), 4, 0);
        let channels = |p: &Piece| {
            let mut v: Vec<u8> = p.score.notes().map(|n| n.channel).collect();
            v.sort();
            v.dedup();
            v
        };
        assert_eq!(channels(&sparse), vec![0, 1]);
        assert_eq!(sparse.phrases[0].pattern, "hold");
        assert!(sparse.score.notes().all(|n| n.dur == Pos::new(1, 1)));

        let busy = generate(1, &Trajectory::flat(dials(0.7, 0.7, 0.9, 0.6)), 4, 0);
        assert_eq!(channels(&busy), vec![0, 1, 2, 3]);
        assert_eq!(busy.phrases[0].pattern, "triplets");
        // Two chords a bar at this pace, so a triplet onset lasts a sixth.
        assert!(busy
            .score
            .notes()
            .filter(|n| n.channel == Role::Bass.channel())
            .all(|n| n.dur == Pos::new(1, 6)));
        // High tension asks for sevenths.
        assert!(busy.phrases[0]
            .chords
            .iter()
            .all(|c| c.chord.tones.len() == 4));
    }

    #[test]
    fn templates_loop_when_they_divide_the_phrase_and_stretch_when_they_do_not() {
        let axis = TEMPLATES.iter().find(|t| t.name == "axis").unwrap();
        let tiled: Vec<u8> = (0..8).map(|j| degree_for_slot(axis, j, 8)).collect();
        assert_eq!(tiled, vec![0, 4, 5, 3, 0, 4, 5, 3]);
        let two_five_one = TEMPLATES.iter().find(|t| t.name == "two-five-one").unwrap();
        let stretched: Vec<u8> = (0..4)
            .map(|j| degree_for_slot(two_five_one, j, 4))
            .collect();
        assert_eq!(stretched, vec![1, 1, 4, 0]);
        let circle = TEMPLATES.iter().find(|t| t.name == "circle").unwrap();
        // Cut short, a template keeps its ending: ii–V–I in two slots is V I.
        let tail: Vec<u8> = (0..2)
            .map(|j| degree_for_slot(two_five_one, j, 2))
            .collect();
        assert_eq!(tail, vec![4, 0]);
        assert_eq!(
            (0..4)
                .map(|j| degree_for_slot(circle, j, 4))
                .collect::<Vec<_>>(),
            vec![5, 1, 4, 0]
        );
        assert_eq!(
            (0..8)
                .map(|j| degree_for_slot(circle, j, 8))
                .collect::<Vec<_>>(),
            circle.degrees
        );
        // A one-bar piece still generates (no template is that short).
        let tiny = generate(1, &Trajectory::flat(dials(0.5, 0.5, 0.5, 0.2)), 1, 0);
        assert_eq!(tiny.phrases[0].chords.len(), 1);
    }

    #[test]
    fn high_tension_is_reached_by_lifting_and_low_tension_is_left_alone() {
        let hot = generate(5, &Trajectory::flat(dials(0.9, 0.6, 0.5, 0.5)), 8, 0);
        assert!(
            hot.phrases.iter().all(|p| p.achieved_tension >= 0.55),
            "{:?}",
            hot.phrases
                .iter()
                .map(|p| p.achieved_tension)
                .collect::<Vec<_>>()
        );
        assert!(hot.phrases.iter().map(|p| p.lifts).sum::<usize>() > 0);
        assert!(hot.phrases[0].numerals().contains('*'));
        // Every phrase still resolves: its final chord is never lifted.
        assert!(hot.phrases.iter().all(|p| !p.chords.last().unwrap().lifted));
        let cool = generate(5, &Trajectory::flat(dials(0.1, 0.6, 0.5, 0.5)), 8, 0);
        // A moderate target is met by template choice alone, never by lifting.
        let mild = generate(5, &Trajectory::flat(dials(0.4, 0.6, 0.5, 0.5)), 8, 0);
        assert!(mild.phrases.iter().all(|p| p.lifts == 0));
        assert!(cool
            .phrases
            .iter()
            .all(|p| p.achieved_tension <= 0.3 && p.lifts == 0));
    }

    #[test]
    fn tempo_follows_pace_and_the_chart_is_readable() {
        let flat = generate(3, &Trajectory::flat(dials(0.3, 0.5, 0.5, 0.5)), 8, 2);
        assert_eq!(flat.score.tempo.len(), 1);
        assert!((flat.score.tempo[0].bpm() - 110.0).abs() < 1e-6);
        let ramp = Trajectory::new(vec![
            (Pos::ZERO, dials(0.3, 0.5, 0.5, 0.0)),
            (Pos::new(8, 1), dials(0.3, 0.5, 0.5, 1.0)),
        ]);
        let moving = generate(3, &ramp, 8, 2);
        assert!(moving.score.tempo.len() >= 4);
        assert!(!flat.phrases[0].numerals().is_empty());
        assert!(role_chart(0.7).contains("flute"));
        assert!(role_chart(0.2).contains("clarinet"));
    }
}
