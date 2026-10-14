//! The melody layer: a MOTIF under the rule of threes (Aaron's ruling).
//!
//! A phrase is four UNITS (a chord slot at one chord per bar, a bar at two).
//! Units one to three state the motif — the same rhythm cell and the same
//! relative contour, re-fitted to each unit's chord — each repeat carrying one
//! small emotive variation. The fourth unit DEPARTS in the direction the
//! harmony is moving: ascending into a rise in tension, descending to the root
//! on a resolution, landing on the tonic at the end. The motif persists while
//! the progression persists and is redrawn when it changes; with four phrases
//! on one motif the fourth phrase departs too. Every choice is a row and every
//! draw is seeded.

use crate::mood::{Phrase, Role, MELODY_DENSITY};
use crate::score::{Event, Note, Score};
use crate::theory::{Chord, Mode};
use crate::time::Pos;

/// A rhythm cell over one unit, in TWELFTHS of the unit: (start, length).
/// Gaps are rests; a length may end before the next start.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cell {
    pub name: &'static str,
    pub notes: &'static [(i64, i64)],
}

pub const CELLS: [Cell; 11] = [
    Cell {
        name: "long",
        notes: &[(0, 12)],
    },
    Cell {
        name: "dotted",
        notes: &[(0, 9), (9, 3)],
    },
    Cell {
        name: "anticipation",
        notes: &[(0, 3), (3, 9)],
    },
    Cell {
        name: "offbeat",
        notes: &[(3, 3), (6, 6)],
    },
    Cell {
        name: "two-and-rest",
        notes: &[(0, 3), (3, 3)],
    },
    Cell {
        name: "halves",
        notes: &[(0, 6), (6, 6)],
    },
    Cell {
        name: "gallop",
        notes: &[(0, 6), (6, 3), (9, 3)],
    },
    Cell {
        name: "run",
        notes: &[(0, 3), (3, 3), (6, 3), (9, 3)],
    },
    Cell {
        name: "triplets",
        notes: &[(0, 4), (4, 4), (8, 4)],
    },
    Cell {
        name: "pickup",
        notes: &[(9, 3)],
    },
    Cell {
        name: "breath",
        notes: &[],
    },
];

pub fn cell(name: &str) -> &'static Cell {
    CELLS
        .iter()
        .find(|c| c.name == name)
        .unwrap_or_else(|| panic!("no melody cell named {name}"))
}

/// Which cells a motif may be drawn from, by density tier.
pub const SPARSE: &[&str] = &["long", "dotted", "halves", "two-and-rest", "breath"];
pub const MEDIUM: &[&str] = &[
    "dotted",
    "anticipation",
    "halves",
    "gallop",
    "offbeat",
    "run",
];
pub const BUSY: &[&str] = &[
    "run",
    "triplets",
    "gallop",
    "anticipation",
    "dotted",
    "offbeat",
];

pub fn eligible_cells(density: f32) -> &'static [&'static str] {
    if density < 0.65 {
        SPARSE
    } else if density < 0.8 {
        MEDIUM
    } else {
        BUSY
    }
}

/// Scale-step offsets from the unit's anchor for the first four notes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Contour {
    pub name: &'static str,
    pub steps: [i8; 4],
}

pub const CONTOURS: [Contour; 6] = [
    Contour {
        name: "arch",
        steps: [0, 2, 1, 0],
    },
    Contour {
        name: "rise",
        steps: [0, 1, 2, 3],
    },
    Contour {
        name: "fall",
        steps: [0, -1, -2, -3],
    },
    Contour {
        name: "wave",
        steps: [0, 1, -1, 0],
    },
    Contour {
        name: "leap-back",
        steps: [0, 4, 3, 2],
    },
    Contour {
        name: "hold",
        steps: [0, 0, 0, 0],
    },
];

pub fn contour(name: &str) -> &'static Contour {
    CONTOURS
        .iter()
        .find(|c| c.name == name)
        .unwrap_or_else(|| panic!("no contour named {name}"))
}

/// The emotive variation a repeated statement carries.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Variation {
    None,
    /// The second note moves one scale step.
    Neighbour,
    /// The last note stretches to the unit's end, or shortens if it already fills it.
    EndChange,
    /// The whole statement steps louder or softer with the tension slope.
    Dynamic,
}

/// Draw weights for statements two and three.
pub const VARIATIONS: [(Variation, f32); 4] = [
    (Variation::None, 0.25),
    (Variation::Neighbour, 0.3),
    (Variation::EndChange, 0.25),
    (Variation::Dynamic, 0.2),
];

/// Where the fourth statement goes, read from the harmony ahead.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Departure {
    /// Tension rises next: climb into a tone of the coming chord.
    Ascend,
    /// The harmony resolves: fall to the chord root on a long value.
    Descend,
    /// End of the piece: land on the tonic and hold.
    Land,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Motif {
    pub cell: &'static str,
    pub contour: &'static str,
}

/// What the melody did in one phrase — for the chart and the tests.
#[derive(Clone, Debug, PartialEq)]
pub struct MelodyPlan {
    pub motif: Motif,
    /// One per statement unit, the first always `None`.
    pub variations: Vec<Variation>,
    pub departure: Departure,
    pub departure_cell: &'static str,
    /// Fourth phrase on one motif: this phrase states a contrasting motif.
    pub phrase_departure: bool,
}

/// Composes phrase after phrase, remembering the motif.
#[derive(Debug)]
pub struct Composer {
    motif: Option<Motif>,
    key: Option<(&'static str, Mode)>,
    run: usize,
    /// The channel the notes land on.
    channel: u8,
    /// Register offset from the pad's centre: +12 for the top-line melody, 0 for a mid-range horn.
    register: i32,
    /// A fixed cell set, or `None` to follow the density tiers.
    cells: Option<&'static [&'static str]>,
    /// Below this density the layer stays silent.
    min_density: f32,
}

impl Default for Composer {
    fn default() -> Self {
        Composer::for_role(Role::Melody, 12, None, MELODY_DENSITY)
    }
}

/// Sounding chord of `phrase` at `at`.
fn chord_at(phrase: &Phrase, at: Pos) -> &Chord {
    phrase
        .chords
        .iter()
        .rev()
        .find(|c| c.at <= at)
        .map_or(&phrase.chords[0].chord, |c| &c.chord)
}

/// Move `pitch` by `n` scale steps of `mode` on `tonic`, octaves wrapping.
pub fn scale_step(mode: Mode, tonic: u8, pitch: u8, n: i32) -> u8 {
    let steps = mode.steps();
    let rel = (pitch as i32 - tonic as i32).rem_euclid(12) as u8;
    let octave_base = pitch as i32 - rel as i32;
    // Snap down to the nearest scale tone, then step.
    let i = steps.iter().rposition(|s| *s <= rel).unwrap_or(0) as i32;
    let j = i + n;
    let out = octave_base + 12 * j.div_euclid(7) + steps[j.rem_euclid(7) as usize] as i32;
    out.clamp(0, 127) as u8
}

/// Nearest pitch within two semitones whose class is in `pcs` (classes are
/// semitones above the tonic); the pitch itself when none is.
fn snap(pitch: u8, tonic: u8, pcs: &[u8]) -> u8 {
    let in_set = |p: i32| pcs.contains(&((p - tonic as i32).rem_euclid(12) as u8));
    for delta in [0, -1, 1, -2, 2] {
        let p = pitch as i32 + delta;
        if (0..=127).contains(&p) && in_set(p) {
            return p as u8;
        }
    }
    pitch
}

/// Nearest pitch (up to a tritone away) whose class is in `pcs` — the strong
/// form of [`snap`], for a note that MUST land on its target.
fn nearest(pitch: u8, tonic: u8, pcs: &[u8]) -> u8 {
    let in_set = |p: i32| pcs.contains(&((p - tonic as i32).rem_euclid(12) as u8));
    for delta in 0..=6 {
        for p in [pitch as i32 - delta, pitch as i32 + delta] {
            if (0..=127).contains(&p) && in_set(p) {
                return p as u8;
            }
        }
    }
    pitch
}

/// Fold `pitch` by octaves into [lo, hi].
fn fold(pitch: u8, lo: i32, hi: i32) -> u8 {
    let mut p = pitch as i32;
    while p > hi {
        p -= 12;
    }
    while p < lo {
        p += 12;
    }
    p.clamp(0, 127) as u8
}

fn pick<'a>(rng: &mut fastrand::Rng, names: &[&'a str], exclude: Option<&str>) -> &'a str {
    let pool: Vec<&str> = names
        .iter()
        .copied()
        .filter(|n| Some(*n) != exclude)
        .collect();
    let pool = if pool.is_empty() {
        names.to_vec()
    } else {
        pool
    };
    pool[rng.usize(..pool.len())]
}

fn draw_variation(rng: &mut fastrand::Rng) -> Variation {
    let total: f32 = VARIATIONS.iter().map(|(_, w)| w).sum();
    let mut x = rng.f32() * total;
    for (v, w) in VARIATIONS {
        x -= w;
        if x <= 0.0 {
            return v;
        }
    }
    Variation::None
}

impl Composer {
    pub fn new() -> Self {
        Self::default()
    }

    /// A composer for `role`: notes on its channel, anchored `register`
    /// semitones from the pad's centre, drawing from `cells` (or the density
    /// tiers), silent below `min_density`.
    pub fn for_role(
        role: Role,
        register: i32,
        cells: Option<&'static [&'static str]>,
        min_density: f32,
    ) -> Self {
        Composer {
            motif: None,
            key: None,
            run: 0,
            channel: role.channel(),
            register,
            cells,
            min_density,
        }
    }

    /// Write the melody for `phrase` into `score`, looking at `next` to shape
    /// the departure. `None` when density keeps the melody silent throughout.
    pub fn phrase(
        &mut self,
        rng: &mut fastrand::Rng,
        score: &mut Score,
        tonic: u8,
        phrase: &Phrase,
        next: Option<&Phrase>,
    ) -> Option<MelodyPlan> {
        let mode = phrase.mode;
        let unit_slots = phrase.unit_slots.max(1);
        let units = phrase.chords.len() / unit_slots;
        if units == 0
            || !phrase
                .chords
                .iter()
                .any(|c| c.dials.density >= self.min_density)
        {
            return None;
        }
        let density = phrase.chords[0].dials.density;

        // The motif: kept while the progression holds, redrawn when it changes;
        // the fourth phrase on one motif states a contrasting one.
        let key = (phrase.template, mode);
        if self.key != Some(key) {
            let previous = self.motif.map(|m| m.cell);
            self.motif = Some(Motif {
                cell: pick(rng, self.cells.unwrap_or(eligible_cells(density)), previous),
                contour: pick(rng, &["arch", "rise", "fall", "wave", "leap-back"], None),
            });
            self.key = Some(key);
            self.run = 0;
        } else {
            self.run += 1;
        }
        let base = self.motif.expect("motif drawn above");
        let phrase_departure = self.run % 4 == 3;
        let motif = if phrase_departure {
            Motif {
                cell: pick(
                    rng,
                    self.cells.unwrap_or(eligible_cells(density)),
                    Some(base.cell),
                ),
                contour: pick(
                    rng,
                    &["arch", "rise", "fall", "wave", "leap-back"],
                    Some(base.contour),
                ),
            }
        } else {
            base
        };

        // Departure direction from the harmony ahead.
        let here = phrase
            .chords
            .last()
            .expect("a phrase has chords")
            .chord
            .tension;
        let departure = match next {
            None => Departure::Land,
            Some(n) if n.chords[0].chord.tension > here + 0.1 => Departure::Ascend,
            Some(_) => Departure::Descend,
        };
        let departure_cell = match departure {
            Departure::Ascend => pick(
                rng,
                if density >= 0.8 {
                    &["anticipation", "pickup", "run"]
                } else {
                    &["anticipation", "pickup"]
                },
                Some(motif.cell),
            ),
            Departure::Descend => pick(rng, &["dotted", "two-and-rest", "long"], Some(motif.cell)),
            Departure::Land => "long",
        };

        let mut variations = Vec::with_capacity(units);
        let mut prev_tension = phrase.chords[0].dials.tension;
        for u in 0..units {
            let first = &phrase.chords[u * unit_slots];
            let last = &phrase.chords[u * unit_slots + unit_slots - 1];
            let unit_start = first.at;
            let unit_len = (last.at + last.dur) - unit_start;
            let d = first.dials;
            let is_departure = u + 1 == units && units >= 1;
            let (cell_name, contour_name, variation, target) = if is_departure {
                let target = match departure {
                    Departure::Ascend => next
                        .map(|n| n.chords[0].chord.pitch_classes())
                        .filter(|pcs| pcs.iter().all(|pc| mode.contains(*pc)))
                        .unwrap_or_else(|| last.chord.pitch_classes()),
                    Departure::Descend => vec![last.chord.tones[0] % 12],
                    Departure::Land => vec![0],
                };
                let contour = match departure {
                    Departure::Ascend => "rise",
                    Departure::Descend => "fall",
                    Departure::Land => "hold",
                };
                (departure_cell, contour, Variation::None, Some(target))
            } else {
                let v = if u == 0 {
                    Variation::None
                } else {
                    draw_variation(rng)
                };
                variations.push(v);
                (motif.cell, motif.contour, v, None)
            };
            if d.density < self.min_density {
                prev_tension = d.tension;
                continue;
            }
            let rising = d.tension >= prev_tension;
            prev_tension = d.tension;
            self.emit_unit(
                rng,
                score,
                phrase,
                tonic,
                cell(cell_name),
                contour(contour_name),
                variation,
                unit_start,
                unit_len,
                d.tension,
                d.brightness,
                rising,
                target.as_deref(),
            );
        }

        Some(MelodyPlan {
            motif,
            variations,
            departure,
            departure_cell,
            phrase_departure,
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn emit_unit(
        &mut self,
        rng: &mut fastrand::Rng,
        score: &mut Score,
        phrase: &Phrase,
        tonic: u8,
        cell: &Cell,
        contour: &Contour,
        variation: Variation,
        unit_start: Pos,
        unit_len: Pos,
        tension: f32,
        brightness: f32,
        rising: bool,
        final_target: Option<&[u8]>,
    ) {
        if cell.notes.is_empty() {
            return;
        }
        let mode = phrase.mode;
        let base = 60 + ((brightness - 0.5) * 12.0).round() as i32 + self.register;
        let (lo, hi) = (base - 5, base + 9);
        // The anchor: the chord tone nearest the register's heart.
        let opening = chord_at(phrase, unit_start);
        let anchor = fold(
            snap(base.clamp(0, 127) as u8, tonic, &opening.pitch_classes()),
            lo,
            hi,
        );
        let count = cell.notes.len();
        let dynamic = match variation {
            Variation::Dynamic if rising => 1.12,
            Variation::Dynamic => 0.88,
            _ => 1.0,
        };
        for (k, &(start, len)) in cell.notes.iter().enumerate() {
            let on = unit_start + unit_len.scaled(start, 12);
            let mut dur = unit_len.scaled(len, 12);
            let is_last = k + 1 == count;
            let mut step = contour.steps[k.min(3)] as i32;
            if variation == Variation::Neighbour && k == 1 {
                step += 1;
            }
            let mut pitch = fold(scale_step(mode, tonic, anchor, step), lo, hi);
            let sounding = chord_at(phrase, on);
            let on_slot = phrase.chords.iter().any(|c| c.at == on);
            if k == 0 || on_slot {
                pitch = snap(pitch, tonic, &sounding.pitch_classes());
            }
            if is_last {
                if let Some(target) = final_target {
                    pitch = nearest(pitch, tonic, target);
                }
                if variation == Variation::EndChange {
                    let end = unit_start + unit_len;
                    dur = if on + dur < end {
                        end - on
                    } else {
                        dur.scaled(2, 3)
                    };
                }
            }
            let accent = if k == 0 { 1.0 } else { 0.88 };
            let jitter = rng.f32() * 0.08 - 0.04;
            let velocity = ((0.5 + 0.4 * tension) * accent * dynamic + jitter).clamp(0.05, 1.0);
            score.events.push(Event::Note(Note {
                at: on,
                dur,
                channel: self.channel,
                pitch,
                velocity,
            }));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mood::{generate, Dials, Trajectory};
    use crate::theory::PITCH_NAMES;

    fn dials(t: f32, b: f32, d: f32, p: f32) -> Dials {
        Dials {
            tension: t,
            brightness: b,
            density: d,
            pace: p,
        }
    }

    #[test]
    fn cells_fit_their_unit_and_every_tier_has_cells() {
        for c in CELLS {
            let mut cursor = 0;
            for &(start, len) in c.notes {
                assert!(start >= cursor, "{} overlaps", c.name);
                assert!(len > 0 && start + len <= 12, "{} leaves its unit", c.name);
                cursor = start + len;
            }
        }
        for tier in [SPARSE, MEDIUM, BUSY] {
            assert!(!tier.is_empty());
            for name in tier {
                let _ = cell(name);
            }
        }
        for c in CONTOURS {
            let _ = contour(c.name);
        }
    }

    #[test]
    fn scale_steps_move_by_degree_and_wrap_octaves() {
        assert_eq!(scale_step(Mode::Ionian, 0, 60, 1), 62);
        assert_eq!(scale_step(Mode::Ionian, 0, 71, 1), 72);
        assert_eq!(scale_step(Mode::Ionian, 0, 60, -1), 59);
        assert_eq!(scale_step(Mode::Ionian, 0, 60, 7), 72);
        assert_eq!(scale_step(Mode::Aeolian, 9, 69, 2), 72);
        // A non-scale pitch snaps down first.
        assert_eq!(scale_step(Mode::Ionian, 0, 61, 1), 62);
        assert_eq!(snap(61, 0, &[0, 4, 7]), 60);
        assert_eq!(snap(66, 0, &[0, 4, 7]), 67);
        assert_eq!(snap(64, 7, &[0]), 64, "snap gives up beyond two semitones");
        assert_eq!(nearest(64, 7, &[0]), 67, "nearest does not");
    }

    /// Melody notes of a piece grouped by bar: (onset, length) relative to the bar.
    fn melody_bars(piece: &crate::mood::Piece, bars: u32) -> Vec<Vec<(Pos, Pos)>> {
        (0..bars)
            .map(|b| {
                let start = Pos::new(b as i64, 1);
                let end = Pos::new(b as i64 + 1, 1);
                let mut v: Vec<(Pos, Pos)> = piece
                    .score
                    .notes()
                    .filter(|n| n.channel == Role::Melody.channel() && n.at >= start && n.at < end)
                    .map(|n| (n.at - start, n.dur))
                    .collect();
                v.sort();
                v
            })
            .collect()
    }

    #[test]
    fn three_statements_share_their_onsets_and_the_fourth_departs() {
        // One chord per bar: a unit is a bar, a phrase is four units.
        for seed in 1..=6u64 {
            let piece = generate(seed, &Trajectory::flat(dials(0.4, 0.7, 0.7, 0.3)), 8, 0);
            let bars = melody_bars(&piece, 8);
            for phrase in &piece.phrases {
                let plan = phrase
                    .melody
                    .as_ref()
                    .expect("melody plays at this density");
                let b = phrase.start_bar as usize;
                let onsets = |i: usize| bars[i].iter().map(|(on, _)| *on).collect::<Vec<_>>();
                assert_eq!(
                    onsets(b),
                    onsets(b + 1),
                    "seed {seed}: statement 2 changed the rhythm"
                );
                assert_eq!(
                    onsets(b),
                    onsets(b + 2),
                    "seed {seed}: statement 3 changed the rhythm"
                );
                assert_ne!(
                    bars[b],
                    bars[b + 3],
                    "seed {seed}: the fourth unit did not depart"
                );
                assert_ne!(plan.departure_cell, plan.motif.cell);
                assert_eq!(plan.variations.len(), 3);
                assert_eq!(plan.variations[0], Variation::None);
            }
        }
    }

    #[test]
    fn rests_and_dotted_values_appear_and_phrases_end_long_on_the_tonic() {
        let mut saw_rest = false;
        let mut saw_dotted = false;
        for seed in 1..=12u64 {
            let piece = generate(seed, &Trajectory::flat(dials(0.3, 0.6, 0.7, 0.3)), 8, 7);
            let bars = melody_bars(&piece, 8);
            let unit = Pos::new(1, 1);
            for bar in &bars {
                let filled: Pos = bar.iter().fold(Pos::ZERO, |acc, (_, d)| acc + *d);
                if filled < unit {
                    saw_rest = true;
                }
                if bar.iter().any(|(_, d)| *d == unit.scaled(3, 4)) {
                    saw_dotted = true;
                }
            }
            // The piece resolves: its last melody note holds the tonic.
            let last = piece
                .score
                .notes()
                .filter(|n| n.channel == Role::Melody.channel())
                .max_by_key(|n| n.at)
                .unwrap();
            assert_eq!(
                (last.pitch as i32 - piece.tonic as i32).rem_euclid(12),
                0,
                "seed {seed} ends off the tonic ({})",
                PITCH_NAMES[last.pitch as usize % 12]
            );
            assert!(last.dur >= Pos::new(1, 2), "seed {seed} ends short");
            assert_eq!(
                piece
                    .phrases
                    .last()
                    .unwrap()
                    .melody
                    .as_ref()
                    .unwrap()
                    .departure,
                Departure::Land
            );
        }
        assert!(saw_rest, "no rests in twelve seeds");
        assert!(saw_dotted, "no dotted values in twelve seeds");
    }

    #[test]
    fn the_motif_follows_the_progression_and_the_fourth_phrase_departs() {
        let piece = generate(4, &Trajectory::flat(dials(0.3, 0.7, 0.7, 0.3)), 16, 0);
        let plans: Vec<&MelodyPlan> = piece
            .phrases
            .iter()
            .map(|p| p.melody.as_ref().unwrap())
            .collect();
        for (a, b) in piece.phrases.iter().zip(piece.phrases.iter().skip(1)) {
            let (pa, pb) = (a.melody.as_ref().unwrap(), b.melody.as_ref().unwrap());
            if a.template == b.template && a.mode == b.mode && !pb.phrase_departure {
                assert_eq!(
                    pa.motif, pb.motif,
                    "the motif changed without the progression changing"
                );
            }
            if a.template != b.template || a.mode != b.mode {
                assert_ne!(
                    pa.motif.cell, pb.motif.cell,
                    "the progression changed but the motif did not"
                );
            }
        }
        // Drive the composer directly: four phrases on one progression.
        let mut composer = Composer::new();
        let mut rng = fastrand::Rng::with_seed(9);
        let mut score = Score::default();
        let phrase = &piece.phrases[0];
        let runs: Vec<MelodyPlan> = (0..4)
            .map(|_| {
                composer
                    .phrase(&mut rng, &mut score, 0, phrase, Some(phrase))
                    .unwrap()
            })
            .collect();
        assert!(runs[..3].iter().all(|p| !p.phrase_departure));
        assert!(runs[3].phrase_departure);
        assert_ne!(runs[3].motif.cell, runs[0].motif.cell);
        assert_eq!(runs[0].motif, runs[1].motif);
        assert!(plans
            .iter()
            .all(|p| p.variations.first() == Some(&Variation::None)));
    }

    #[test]
    fn a_rise_ahead_makes_the_departure_climb() {
        // Calm for four bars, then a step up to near-maximum tension at bar 5.
        let tr = Trajectory::new(vec![
            (Pos::ZERO, dials(0.1, 0.7, 0.7, 0.3)),
            (Pos::new(15, 4), dials(0.1, 0.7, 0.7, 0.3)),
            (Pos::new(4, 1), dials(0.95, 0.7, 0.7, 0.3)),
        ]);
        let piece = generate(2, &tr, 8, 0);
        let first = piece.phrases[0].melody.as_ref().unwrap();
        assert_eq!(first.departure, Departure::Ascend);
        assert!(["anticipation", "pickup", "run"].contains(&first.departure_cell));
    }
}
