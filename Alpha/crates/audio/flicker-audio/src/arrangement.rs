//! The FORM engine: sections and layers over time (Aaron's layered-movements
//! ruling). A form is rows of sections; each section says how long it lasts,
//! which layers play, how slowly the harmony moves, and how its layers fade in
//! and out. The harmony (template, lift, voicing), the melody composer and the
//! role rows are the same parts the dial-driven loop used — the loop is simply
//! the one-section form.

use std::ops::Range;

use crate::melody::{Composer, MelodyPlan, SPARSE};
use crate::mood::{
    bpm_for, pattern_for, push_note, realize, tones_for, voice, Dials, Phrase, Piece, PlacedChord,
    Role, Trajectory, COLOUR_DENSITY, MELODY_DENSITY, ROLES,
};
use crate::score::{Control, ControlKind, Event, Meter, Program, Score};
use crate::theory::{diatonic_chord, Chord, Mode, Template, TEMPLATES};
use crate::time::{Pos, TempoChange};

/// How a section moves harmonically.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Harmony {
    /// The tonic alone — a drone, no progression.
    Tonic,
    /// Each chord held this many bars; four chords make a phrase.
    Bars(u32),
    /// One chord per bar, two when pace ≥ 0.5 — the dial-driven loop.
    Pace,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Section {
    pub name: &'static str,
    pub bars: u32,
    pub layers: Vec<Role>,
    pub harmony: Harmony,
    /// Pick a new progression template at this section's start.
    pub shift: bool,
    /// Re-pick the template every phrase (the loop's variety).
    pub reselect: bool,
    /// Melody and colour obey the density dial; otherwise listed layers always play.
    pub gated: bool,
    /// Bars over which a layer entering here swells from silence.
    pub fade_in: u32,
    /// Bars over which a layer leaving after this section dies away.
    pub fade_out: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Form {
    pub name: &'static str,
    pub sections: Vec<Section>,
}

impl Form {
    /// Aaron's sketch: a fading drone, a ringing triplet, the progression
    /// looped twice, then horn melody with root bass and strings, then release.
    pub fn ambient() -> Form {
        let s = |name, bars, layers: &[Role], harmony, shift, fade_in, fade_out| Section {
            name,
            bars,
            layers: layers.to_vec(),
            harmony,
            shift,
            reselect: false,
            gated: false,
            fade_in,
            fade_out,
        };
        Form {
            name: "ambient",
            sections: vec![
                s("drone", 4, &[Role::Drone], Harmony::Tonic, false, 2, 1),
                s("shimmer", 4, &[Role::Colour], Harmony::Tonic, false, 0, 0),
                s(
                    "progression",
                    16,
                    &[Role::Pad, Role::Strings, Role::Colour],
                    Harmony::Bars(2),
                    true,
                    2,
                    0,
                ),
                s(
                    "melody",
                    16,
                    &[Role::Pad, Role::Strings, Role::Bass, Role::Horn],
                    Harmony::Bars(2),
                    false,
                    1,
                    0,
                ),
                s(
                    "crescendo",
                    8,
                    &[
                        Role::Pad,
                        Role::Strings,
                        Role::Bass,
                        Role::Horn,
                        Role::Colour,
                        Role::March,
                        Role::Timpani,
                    ],
                    Harmony::Bars(2),
                    false,
                    6,
                    1,
                ),
                s(
                    "release",
                    8,
                    &[Role::Pad, Role::Drone],
                    Harmony::Bars(4),
                    false,
                    2,
                    2,
                ),
                s("fade", 4, &[Role::Drone], Harmony::Tonic, false, 0, 4),
            ],
        }
    }

    /// The ambient form under a breezy moonlit sky: breeze and crickets
    /// throughout, frogs from the shimmer to the release.
    pub fn night() -> Form {
        let mut form = Form::ambient();
        form.name = "night";
        let last = form.sections.len() - 1;
        for (i, section) in form.sections.iter_mut().enumerate() {
            section.layers.push(Role::Breeze);
            section.layers.push(Role::Crickets);
            if (1..last).contains(&i) {
                section.layers.push(Role::Frogs);
            }
            if i == last {
                section.fade_out = section.fade_out.max(4);
            }
        }
        form
    }

    /// The dial-driven loop: every layer by density, a fresh template each phrase.
    pub fn looped(bars: u32) -> Form {
        Form {
            name: "loop",
            sections: vec![Section {
                name: "loop",
                bars: bars.max(1),
                layers: vec![Role::Pad, Role::Bass, Role::Melody, Role::Colour],
                harmony: Harmony::Pace,
                shift: true,
                reselect: true,
                gated: true,
                fade_in: 0,
                fade_out: 0,
            }],
        }
    }

    pub fn parse(name: &str, bars: u32) -> Option<Form> {
        match name {
            "ambient" => Some(Form::ambient()),
            "night" => Some(Form::night()),
            "loop" => Some(Form::looped(bars)),
            _ => None,
        }
    }

    pub fn bars(&self) -> u32 {
        self.sections.iter().map(|s| s.bars).sum()
    }
}

/// Where a section landed in the piece, for the chart.
#[derive(Clone, Debug, PartialEq)]
pub struct PlacedSection {
    pub name: &'static str,
    pub start_bar: u32,
    pub bars: u32,
    pub layers: Vec<Role>,
    pub harmony: Harmony,
    pub phrases: Range<usize>,
}

/// Templates scoring within this much of the best are a seeded choice; wider
/// means more variety, narrower means the tension target rules.
pub const NEAR_TIE: f32 = 0.04;

/// One bar of march percussion: (start numerator, denominator, kit piece,
/// velocity). Piece 88 is the roll, which lasts three beats.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MarchBar {
    pub name: &'static str,
    pub hits: &'static [(i64, i64, u8, f32)],
}

/// The marching bar: bass drum on one and three, snare on the quarters with
/// eighth fills, a soft ruff into the next downbeat.
pub const MARCH_CADENCE: MarchBar = MarchBar {
    name: "cadence",
    hits: &[
        (0, 1, 36, 0.9),
        (0, 1, 38, 0.8),
        (1, 4, 38, 0.6),
        (3, 8, 38, 0.55),
        (1, 2, 36, 0.8),
        (1, 2, 38, 0.75),
        (5, 8, 38, 0.5),
        (3, 4, 38, 0.7),
        (7, 8, 38, 0.45),
        (15, 16, 38, 0.5),
    ],
};

/// The bar that closes a phrase: a roll swelling through three beats, an accent.
pub const MARCH_ROLL: MarchBar = MarchBar {
    name: "roll",
    hits: &[
        (0, 1, 36, 0.8),
        (0, 1, 88, 0.7),
        (3, 4, 38, 0.9),
        (7, 8, 38, 0.6),
    ],
};

/// The bar that opens a phrase: crash, bass drum and snare together.
pub const MARCH_HIT: MarchBar = MarchBar {
    name: "hit",
    hits: &[
        (0, 1, 49, 0.9),
        (0, 1, 36, 1.0),
        (0, 1, 38, 0.95),
        (1, 2, 36, 0.8),
        (1, 2, 38, 0.7),
        (3, 4, 38, 0.6),
    ],
};

/// Which march bar plays at bar `b` of a phrase of `len` bars.
pub fn march_bar(b: u32, len: u32) -> &'static MarchBar {
    if len > 1 && b + 1 == len {
        &MARCH_ROLL
    } else if b == 0 {
        &MARCH_HIT
    } else {
        &MARCH_CADENCE
    }
}

fn expression(score: &mut Score, role: Role, at: Pos, value: f32) {
    score.events.push(Event::Control(Control {
        at,
        channel: role.channel(),
        kind: ControlKind::Expression,
        value,
    }));
}

/// Render `form` from the trajectory. Pure in (form, seed, trajectory, tonic).
pub fn compose(form: &Form, seed: u64, trajectory: &Trajectory, tonic: u8) -> Piece {
    let mut rng = fastrand::Rng::with_seed(seed);
    let tonic = tonic % 12 + 48;
    let total_bars = form.bars().max(1);
    let mut score = Score {
        meter: vec![Meter {
            at: Pos::ZERO,
            numerator: 4,
            denominator: 4,
        }],
        ..Score::default()
    };

    // Role bindings and strip settings at the origin.
    let opening = trajectory.at(Pos::ZERO);
    for role in ROLES {
        let ch = role.channel();
        score.events.push(Event::Program(Program {
            at: Pos::ZERO,
            channel: ch,
            patch: role.program(opening.brightness),
        }));
        score.events.push(Event::Control(Control {
            at: Pos::ZERO,
            channel: ch,
            kind: ControlKind::Volume,
            value: role.volume(),
        }));
        score.events.push(Event::Control(Control {
            at: Pos::ZERO,
            channel: ch,
            kind: ControlKind::Pan,
            value: role.pan(),
        }));
        score.events.push(Event::Control(Control {
            at: Pos::ZERO,
            channel: ch,
            kind: ControlKind::Reverb,
            value: role.reverb(),
        }));
    }

    // Tempo rows wherever the pace dial moves the bpm by a whole beat.
    let mut last_bpm = f64::NAN;
    for bar in 0..total_bars {
        let at = Pos::new(bar as i64, 1);
        let bpm = bpm_for(trajectory.at(at).pace).round();
        if last_bpm.is_nan() || (bpm - last_bpm).abs() >= 1.0 {
            score.tempo.push(TempoChange {
                at,
                whole_secs: 240.0 / bpm,
            });
            last_bpm = bpm;
        }
    }

    // Fades: a layer entering a section swells in; a layer absent from the
    // next section dies away at this one's end. Rows on the Expression lane.
    let mut start_bar = 0u32;
    for (i, section) in form.sections.iter().enumerate() {
        let start = Pos::new(start_bar as i64, 1);
        let end_bar = start_bar + section.bars;
        let end = Pos::new(end_bar as i64, 1);
        for role in &section.layers {
            let was = i > 0 && form.sections[i - 1].layers.contains(role);
            if !was {
                if section.fade_in > 0 {
                    expression(&mut score, *role, start, 0.0);
                    expression(
                        &mut score,
                        *role,
                        Pos::new((start_bar + section.fade_in.min(section.bars)) as i64, 1),
                        1.0,
                    );
                } else {
                    expression(&mut score, *role, start, 1.0);
                }
            }
            let stays = i + 1 < form.sections.len() && form.sections[i + 1].layers.contains(role);
            if !stays && section.fade_out > 0 {
                expression(
                    &mut score,
                    *role,
                    Pos::new((end_bar - section.fade_out.min(section.bars)) as i64, 1),
                    1.0,
                );
                expression(&mut score, *role, end, 0.0);
            }
        }
        start_bar = end_bar;
    }

    // Harmony and the sounding layers, section by section.
    let mut phrases: Vec<Phrase> = Vec::new();
    let mut sections: Vec<PlacedSection> = Vec::new();
    let mut template: Option<&'static Template> = None;
    let mut prev_voicing: Option<Vec<u8>> = None;
    let mut bar = 0u32;
    for section in &form.sections {
        let first_phrase = phrases.len();
        let section_end = bar + section.bars;
        let mut first_in_section = true;
        while bar < section_end {
            let start = Pos::new(bar as i64, 1);
            let opening = trajectory.at(start);
            let mode = Mode::for_brightness(opening.brightness);
            let remaining = section_end - bar;
            // Phrase geometry: slots and their length.
            let (len, slots, slot_len, per_bar) = match section.harmony {
                Harmony::Tonic => {
                    let len = remaining.min(4);
                    (len, 1i64, Pos::new(len as i64, 1), 1usize)
                }
                Harmony::Bars(n) => {
                    let n = n.max(1);
                    let len = remaining.min(4 * n);
                    let slots = (len / n).max(1) as i64;
                    (len, slots, Pos::new(n as i64, 1), 1usize)
                }
                Harmony::Pace => {
                    let per_bar: i64 = if opening.pace >= 0.5 { 2 } else { 1 };
                    let len = remaining.min(4);
                    (
                        len,
                        len as i64 * per_bar,
                        Pos::new(1, per_bar),
                        per_bar as usize,
                    )
                }
            };
            let slot_at = |j: i64| start + slot_len.scaled(j, 1);
            let dials: Vec<Dials> = (0..slots).map(|j| trajectory.at(slot_at(j))).collect();

            // The chords: the tonic alone, or a template (kept, shifted or re-picked).
            let (realized, template_name): (Vec<(Chord, bool)>, &'static str) = if section.harmony
                == Harmony::Tonic
            {
                let d = dials[0];
                (
                    vec![(
                        diatonic_chord(mode, 0, tones_for(d.tension, d.density)),
                        false,
                    )],
                    "tonic",
                )
            } else {
                let pick =
                    template.is_none() || section.reselect || (section.shift && first_in_section);
                if pick {
                    let mut candidates: Vec<&'static Template> = TEMPLATES
                        .iter()
                        .filter(|t| t.degrees.len() <= slots as usize)
                        .collect();
                    if candidates.is_empty() {
                        candidates = TEMPLATES.iter().collect();
                    }
                    let mut costs: Vec<(f32, usize)> = candidates
                        .iter()
                        .enumerate()
                        .map(|(i, tpl)| {
                            let cost = realize(tpl, mode, &dials)
                                .iter()
                                .zip(&dials)
                                .map(|((c, _), d)| (c.tension - d.tension).abs())
                                .sum::<f32>()
                                / slots as f32;
                            (cost, i)
                        })
                        .collect();
                    costs.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
                    let near = costs
                        .iter()
                        .take_while(|(c, _)| *c <= costs[0].0 + NEAR_TIE)
                        .count();
                    template = Some(candidates[costs[rng.usize(..near)].1]);
                }
                let tpl = template.expect("a template is chosen above");
                (realize(tpl, mode, &dials), tpl.name)
            };
            first_in_section = false;

            // Drone: the tonic held for the whole phrase.
            let phrase_len = Pos::new(len as i64, 1);
            if section.layers.contains(&Role::Drone) {
                push_note(&mut score, Role::Drone, start, phrase_len, tonic, 0.45);
            }
            // Textures: one note per individual for the phrase; pitch is the individual.
            if section.layers.contains(&Role::Breeze) {
                push_note(&mut score, Role::Breeze, start, phrase_len, 60, 0.6);
            }
            if section.layers.contains(&Role::Crickets) {
                for individual in [58u8, 62, 66] {
                    push_note(
                        &mut score,
                        Role::Crickets,
                        start,
                        phrase_len,
                        individual,
                        0.6,
                    );
                }
            }
            if section.layers.contains(&Role::Frogs) {
                for individual in [64u8, 52] {
                    push_note(&mut score, Role::Frogs, start, phrase_len, individual, 0.6);
                }
            }
            // March: a bar row per bar of the phrase — hit, cadences, roll.
            if section.layers.contains(&Role::March) {
                for b in 0..len {
                    let bar_at = start + Pos::new(b as i64, 1);
                    let row = march_bar(b, len);
                    let t = trajectory.at(bar_at).tension;
                    for &(num, den, piece, velocity) in row.hits {
                        let on = bar_at + Pos::new(num, den);
                        let dur = if piece == 88 {
                            Pos::new(3, 4)
                        } else {
                            Pos::new(1, 16)
                        };
                        push_note(
                            &mut score,
                            Role::March,
                            on,
                            dur,
                            piece,
                            velocity * (0.6 + 0.4 * t),
                        );
                    }
                }
            }

            let mut chords = Vec::with_capacity(realized.len());
            for (j, (chord, lifted)) in realized.into_iter().enumerate() {
                let at = slot_at(j as i64);
                let d = dials[j.min(dials.len() - 1)];
                let center = 60 + ((d.brightness - 0.5) * 12.0).round() as i32;
                let voicing = voice(&chord, tonic, center, prev_voicing.as_deref());
                let pattern = pattern_for(d.density);

                if section.layers.contains(&Role::Pad) {
                    for &p in &voicing {
                        push_note(
                            &mut score,
                            Role::Pad,
                            at,
                            slot_len,
                            p,
                            0.35 + 0.3 * d.tension,
                        );
                    }
                }
                if section.layers.contains(&Role::Strings) {
                    for &p in &voicing {
                        push_note(
                            &mut score,
                            Role::Strings,
                            at,
                            slot_len,
                            p.saturating_add(12),
                            0.3 + 0.25 * d.tension,
                        );
                    }
                }
                // Bass: the root — on the density pattern in the gated loop, held
                // for the whole chord everywhere else.
                if section.layers.contains(&Role::Bass) {
                    let root_pc = (tonic as i32 + chord.tones[0] as i32).rem_euclid(12);
                    let bass = (36 + root_pc) as u8;
                    if section.gated {
                        for &(num, den) in pattern.onsets {
                            let on = at + slot_len.scaled(num, den);
                            push_note(
                                &mut score,
                                Role::Bass,
                                on,
                                slot_len.scaled(1, den),
                                bass,
                                0.55 + 0.3 * d.tension,
                            );
                        }
                    } else {
                        push_note(
                            &mut score,
                            Role::Bass,
                            at,
                            slot_len,
                            bass,
                            0.55 + 0.3 * d.tension,
                        );
                    }
                }
                // Timpani: the root struck on each bar's downbeat and third beat.
                if section.layers.contains(&Role::Timpani) {
                    let root_pc = (tonic as i32 + chord.tones[0] as i32).rem_euclid(12);
                    let drum = (36 + root_pc) as u8;
                    let bars_in_slot = (slot_len.num() / slot_len.den().max(1)).max(1);
                    for b in 0..bars_in_slot {
                        let bar_at = at + Pos::new(b, 1);
                        push_note(
                            &mut score,
                            Role::Timpani,
                            bar_at,
                            Pos::new(1, 4),
                            drum,
                            0.6 + 0.3 * d.tension,
                        );
                        push_note(
                            &mut score,
                            Role::Timpani,
                            bar_at + Pos::new(1, 2),
                            Pos::new(1, 4),
                            drum,
                            0.4 + 0.2 * d.tension,
                        );
                    }
                }
                // Colour: a triplet over the first bar of the chord, then left to ring.
                if section.layers.contains(&Role::Colour)
                    && (!section.gated || d.density >= COLOUR_DENSITY)
                {
                    let figure = if slot_len < Pos::new(1, 1) {
                        slot_len
                    } else {
                        Pos::new(1, 1)
                    };
                    for k in 0..3i64 {
                        let on = at + figure.scaled(k, 3);
                        let pitch = voicing[k as usize % voicing.len()].saturating_add(12);
                        push_note(
                            &mut score,
                            Role::Colour,
                            on,
                            figure.scaled(1, 3),
                            pitch,
                            0.3 + 0.2 * d.tension,
                        );
                    }
                }

                prev_voicing = Some(voicing.clone());
                chords.push(PlacedChord {
                    at,
                    dur: slot_len,
                    chord,
                    voicing,
                    lifted,
                    dials: d,
                });
            }

            let n = chords.len() as f32;
            let target = dials.iter().map(|d| d.tension).sum::<f32>() / dials.len() as f32;
            let achieved = chords.iter().map(|c| c.chord.tension).sum::<f32>() / n;
            let lifts = chords.iter().filter(|c| c.lifted).count();
            phrases.push(Phrase {
                start_bar: bar,
                bars: len,
                mode,
                template: template_name,
                pattern: pattern_for(opening.density).name,
                chords,
                target_tension: target,
                achieved_tension: achieved,
                lifts,
                unit_slots: per_bar,
                melody: None,
            });
            bar += len;
        }
        sections.push(PlacedSection {
            name: section.name,
            start_bar: bar - section.bars,
            bars: section.bars,
            layers: section.layers.clone(),
            harmony: section.harmony,
            phrases: first_phrase..phrases.len(),
        });
    }

    // The melody pass: harmony first, so every departure can see the chord
    // it is heading for. The horn is the melody at mid range on sparse cells.
    let mut melody = Composer::new();
    let mut horn = Composer::for_role(Role::Horn, 0, Some(SPARSE), 0.0);
    let mut plans: Vec<Option<MelodyPlan>> = vec![None; phrases.len()];
    for (si, section) in form.sections.iter().enumerate() {
        let composer = if section.layers.contains(&Role::Horn) {
            &mut horn
        } else if section.layers.contains(&Role::Melody) {
            &mut melody
        } else {
            continue;
        };
        for i in sections[si].phrases.clone() {
            plans[i] =
                composer.phrase(&mut rng, &mut score, tonic, &phrases[i], phrases.get(i + 1));
        }
    }
    for (phrase, plan) in phrases.iter_mut().zip(plans) {
        phrase.melody = plan;
    }
    let _ = MELODY_DENSITY;

    score.end = Pos::new(total_bars as i64, 1);
    score.sort();
    Piece {
        score,
        phrases,
        sections,
        tonic: tonic % 12,
        seed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dials(t: f32, b: f32, d: f32, p: f32) -> Dials {
        Dials {
            tension: t,
            brightness: b,
            density: d,
            pace: p,
        }
    }

    fn notes_on(piece: &Piece, role: Role, from_bar: u32, to_bar: u32) -> Vec<(Pos, Pos, u8)> {
        let (from, to) = (Pos::new(from_bar as i64, 1), Pos::new(to_bar as i64, 1));
        piece
            .score
            .notes()
            .filter(|n| n.channel == role.channel() && n.at >= from && n.at < to)
            .map(|n| (n.at, n.dur, n.pitch))
            .collect()
    }

    #[test]
    fn the_ambient_form_lays_its_sections_and_layers_as_sketched() {
        let form = Form::ambient();
        assert_eq!(form.bars(), 60);
        let piece = compose(&form, 7, &Trajectory::flat(dials(0.3, 0.6, 0.3, 0.25)), 0);
        assert_eq!(piece.sections.len(), 7);
        assert_eq!(piece.score.end, Pos::new(60, 1));
        // Bars 1–4: the drone alone, one held tonic note the length of the phrase.
        let drone = notes_on(&piece, Role::Drone, 0, 4);
        assert_eq!(drone, vec![(Pos::ZERO, Pos::new(4, 1), 48)]);
        for role in [
            Role::Pad,
            Role::Strings,
            Role::Bass,
            Role::Horn,
            Role::Colour,
            Role::Melody,
        ] {
            assert!(
                notes_on(&piece, role, 0, 4).is_empty(),
                "{role:?} plays in the opening"
            );
        }
        // Bars 5–8: the drone has faded out; the shimmer triplet rings alone —
        // three notes of a third each, then rest.
        let shimmer = notes_on(&piece, Role::Colour, 4, 8);
        assert_eq!(shimmer.len(), 3);
        assert!(shimmer.iter().all(|(_, dur, _)| *dur == Pos::new(1, 3)));
        assert!(notes_on(&piece, Role::Drone, 4, 8).is_empty());
        // Bars 9–24: the progression on pad and strings, one chord per two bars, looped twice.
        let section = &piece.sections[2];
        assert_eq!(section.phrases.len(), 2);
        let (a, b) = (
            &piece.phrases[section.phrases.start],
            &piece.phrases[section.phrases.start + 1],
        );
        assert_eq!(a.template, b.template);
        let degrees = |p: &Phrase| p.chords.iter().map(|c| c.chord.degree).collect::<Vec<_>>();
        assert_eq!(degrees(a), degrees(b), "the loop did not repeat");
        assert!(a.chords.iter().all(|c| c.dur == Pos::new(2, 1)));
        assert!(!notes_on(&piece, Role::Pad, 8, 24).is_empty());
        assert!(!notes_on(&piece, Role::Strings, 8, 24).is_empty());
        assert!(notes_on(&piece, Role::Bass, 8, 24).is_empty());
        assert!(notes_on(&piece, Role::Horn, 8, 24).is_empty());
        // Bars 25–40: the horn melody and the root bass join on the same progression.
        let melody_section = &piece.sections[3];
        let c = &piece.phrases[melody_section.phrases.start];
        assert_eq!(
            c.template, a.template,
            "the melody section shifted the progression"
        );
        assert!(!notes_on(&piece, Role::Horn, 24, 40).is_empty());
        let bass = notes_on(&piece, Role::Bass, 24, 40);
        assert!(
            bass.iter().all(|(_, dur, _)| *dur == Pos::new(2, 1)),
            "the bass should hold each root"
        );
        assert!(c.melody.is_some());
        // Bars 41–48: the crescendo — march on the kit channel and timpani roots join, only here.
        let march = notes_on(&piece, Role::March, 40, 48);
        assert!(!march.is_empty());
        assert!(notes_on(&piece, Role::March, 0, 40).is_empty());
        assert!(notes_on(&piece, Role::March, 48, 60).is_empty());
        assert!(
            march
                .iter()
                .any(|(_, dur, piece)| *piece == 88 && *dur == Pos::new(3, 4)),
            "no roll"
        );
        assert!(march.iter().any(|(_, _, piece)| *piece == 49), "no crash");
        let timpani = notes_on(&piece, Role::Timpani, 40, 48);
        assert_eq!(timpani.len(), 16, "two timpani strokes a bar");
        assert!(notes_on(&piece, Role::Timpani, 0, 40).is_empty());
        // No textures in the plain ambient form.
        for role in [Role::Breeze, Role::Crickets, Role::Frogs] {
            assert!(notes_on(&piece, role, 0, 60).is_empty());
        }
        // The end: the drone alone, fading.
        assert!(!notes_on(&piece, Role::Drone, 56, 60).is_empty());
        assert!(notes_on(&piece, Role::Pad, 56, 60).is_empty());
    }

    #[test]
    fn fades_are_expression_rows_at_the_section_edges() {
        let piece = compose(
            &Form::ambient(),
            7,
            &Trajectory::flat(dials(0.3, 0.6, 0.3, 0.25)),
            0,
        );
        let lane = |role: Role| -> Vec<(Pos, f32)> {
            piece
                .score
                .events
                .iter()
                .filter_map(|e| match e {
                    Event::Control(c)
                        if c.channel == role.channel() && c.kind == ControlKind::Expression =>
                    {
                        Some((c.at, c.value))
                    }
                    _ => None,
                })
                .collect()
        };
        let drone = lane(Role::Drone);
        // Swells in over two bars, dies over the last bar of the opening,
        // returns over two bars at the release, and fades out at the very end.
        assert!(drone.contains(&(Pos::ZERO, 0.0)));
        assert!(drone.contains(&(Pos::new(2, 1), 1.0)));
        assert!(drone.contains(&(Pos::new(3, 1), 1.0)) && drone.contains(&(Pos::new(4, 1), 0.0)));
        assert!(drone.contains(&(Pos::new(48, 1), 0.0)) && drone.contains(&(Pos::new(50, 1), 1.0)));
        assert!(drone.contains(&(Pos::new(60, 1), 0.0)));
        // The march swells in over six bars of the crescendo and leaves over its last bar.
        let march = lane(Role::March);
        assert!(march.contains(&(Pos::new(40, 1), 0.0)) && march.contains(&(Pos::new(46, 1), 1.0)));
        assert!(march.contains(&(Pos::new(47, 1), 1.0)) && march.contains(&(Pos::new(48, 1), 0.0)));
        // The shimmer starts clean: no fade row at its entry.
        let colour = lane(Role::Colour);
        assert!(
            colour.contains(&(Pos::new(4, 1), 1.0)) && !colour.contains(&(Pos::new(4, 1), 0.0))
        );
        // The horn enters at bar 25 over one bar.
        let horn = lane(Role::Horn);
        assert!(horn.contains(&(Pos::new(24, 1), 0.0)) && horn.contains(&(Pos::new(25, 1), 1.0)));
        // Layers that run straight through get no rows mid-way.
        let pad = lane(Role::Pad);
        assert!(pad
            .iter()
            .all(|(at, _)| *at <= Pos::new(10, 1) || *at >= Pos::new(54, 1)));
    }

    #[test]
    fn the_night_form_adds_the_soundscape() {
        let piece = compose(
            &Form::night(),
            7,
            &Trajectory::flat(dials(0.3, 0.6, 0.3, 0.25)),
            0,
        );
        assert_eq!(piece.score.end, Pos::new(60, 1));
        // Breeze and crickets from the first bar; three cricket individuals per phrase.
        assert_eq!(notes_on(&piece, Role::Breeze, 0, 4).len(), 1);
        assert_eq!(notes_on(&piece, Role::Crickets, 0, 4).len(), 3);
        assert_eq!(notes_on(&piece, Role::Breeze, 56, 60).len(), 1);
        // Frogs sit out the opening and the final fade.
        assert!(notes_on(&piece, Role::Frogs, 0, 4).is_empty());
        assert_eq!(notes_on(&piece, Role::Frogs, 4, 8).len(), 2);
        assert!(notes_on(&piece, Role::Frogs, 56, 60).is_empty());
        // The music underneath is the ambient form's.
        assert_eq!(piece.sections.len(), 7);
        assert!(!notes_on(&piece, Role::March, 40, 48).is_empty());
        assert_eq!(march_bar(0, 4).name, "hit");
        assert_eq!(march_bar(3, 4).name, "roll");
        assert_eq!(march_bar(1, 4).name, "cadence");
        assert!(Form::parse("night", 8).is_some());
    }

    #[test]
    fn the_loop_form_is_the_dial_driven_generator() {
        let tr = Trajectory::flat(dials(0.5, 0.6, 0.6, 0.5));
        let a = compose(&Form::looped(8), 7, &tr, 0);
        let b = crate::mood::generate(7, &tr, 8, 0);
        assert_eq!(a, b);
        assert_eq!(a.sections.len(), 1);
        assert_eq!(a.phrases.len(), 2);
        assert!(Form::parse("ambient", 8).is_some() && Form::parse("loop", 8).is_some());
        assert!(Form::parse("nope", 8).is_none());
    }

    #[test]
    fn every_pitch_stays_in_the_mode_across_the_whole_form() {
        for (seed, b) in [(1u64, 0.2f32), (2, 0.6), (3, 0.9)] {
            let piece = compose(
                &Form::ambient(),
                seed,
                &Trajectory::flat(dials(0.4, b, 0.3, 0.25)),
                5,
            );
            let mode = Mode::for_brightness(b);
            // Kit pieces and texture individuals are not pitches.
            let unpitched = [
                crate::patch::KIT_CHANNEL,
                Role::Breeze.channel(),
                Role::Crickets.channel(),
                Role::Frogs.channel(),
            ];
            for n in piece
                .score
                .notes()
                .filter(|n| !unpitched.contains(&n.channel))
            {
                let pc = (n.pitch as i32 - piece.tonic as i32).rem_euclid(12) as u8;
                assert!(
                    mode.contains(pc),
                    "seed {seed} pitch {} outside {mode:?}",
                    n.pitch
                );
            }
        }
    }
}
