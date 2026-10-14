//! Rows → sound. Walks a [`Score`] once, keeps a strip per channel (patch,
//! volume, expression, pan, bend, sustain), builds one fundsp voice per note
//! from its recipe, and schedules it on a sample-accurate [`Sequencer`] behind
//! a reverb bus and a limiter. The result is one `Net` with no inputs and a
//! stereo output, ready to render offline or feed a device stream.
//!
//! The EXPRESSION lane is automation: a note's envelope is multiplied by the
//! channel's expression curve over the note's own lifetime, so a held pad can
//! swell in and die away from a few rows. Volume, pan and bend are still read
//! at note-on, and the whole piece is scheduled up front (finished voices are
//! freed on the rendering thread; the frontend/backend split lands with the
//! realtime work).

use std::collections::BTreeSet;

use fundsp::prelude32::*;

use crate::patch::{bend_ratio, hz, piece, voice_for, Osc, Piece, Recipe, Voice, KIT_CHANNEL};
use crate::score::Note;
use crate::score::{ControlKind, Event, Score};
use crate::texture::texture_voice;

/// Silence appended after the last note so releases and the reverb finish.
pub const TAIL_SECS: f64 = 2.0;
/// Per-voice level before the strip; leaves headroom for polyphony.
const VOICE_LEVEL: f32 = 0.18;
/// Room reverb mixed into everything on the dry pair.
const REVERB_WET: f32 = 0.12;
/// Air absorption on the way to the distance bus: what far-off sources lose first.
const DISTANCE_CUTOFF: f32 = 2500.0;
/// A recipe's lowpass never opens wider than this many harmonics above the
/// note, so a bright recipe played low stays round instead of buzzing.
pub const MAX_HARMONICS: f32 = 24.0;

/// A score compiled to a playable graph.
pub struct Built {
    /// Zero inputs, two outputs. Sample rate already set.
    pub master: Net,
    /// Seconds from the origin to the end of the tail.
    pub duration_secs: f64,
    /// Everything that resolved to a fallback, one line each.
    pub warnings: Vec<String>,
    /// Voices scheduled.
    pub voices: usize,
}

#[derive(Clone, Copy)]
struct Strip {
    patch: u8,
    volume: f32,
    expression: f32,
    pan: f32,
    bend: f32,
    /// Send into the distance bus, 0..1.
    reverb: f32,
}

impl Default for Strip {
    fn default() -> Self {
        // General MIDI power-on state.
        Strip {
            patch: 0,
            volume: 100.0 / 127.0,
            expression: 1.0,
            pan: 0.0,
            bend: 0.0,
            reverb: 0.0,
        }
    }
}

/// Compile `score` for playback at `sample_rate`.
pub fn build(score: &Score, sample_rate: f64) -> Built {
    // Four outputs per voice: the dry stereo pair and the distance-bus send pair.
    let mut seq = Sequencer::new(0, 4, ReplayMode::None);
    // Voices copy the sequencer's rate when pushed, so set it first.
    seq.set_sample_rate(sample_rate);

    let mut strips = [Strip::default(); 16];
    let sustain = sustain_intervals(score);
    let lanes = expression_lanes(score);
    let mut unmapped: BTreeSet<u8> = BTreeSet::new();
    let mut last_end = score.duration_secs();
    let mut voices = 0usize;

    for event in &score.events {
        match event {
            Event::Program(p) => strips[(p.channel & 0x0F) as usize].patch = p.patch,
            Event::Control(c) => {
                let s = &mut strips[(c.channel & 0x0F) as usize];
                match c.kind {
                    ControlKind::Volume => s.volume = c.value,
                    ControlKind::Expression => s.expression = c.value,
                    ControlKind::Pan => s.pan = c.value,
                    ControlKind::PitchBend => s.bend = c.value,
                    ControlKind::Reverb => s.reverb = c.value,
                    ControlKind::Sustain => {}
                }
            }
            Event::Note(n) => {
                let channel = (n.channel & 0x0F) as usize;
                let s = strips[channel];
                let start = score.seconds_at(n.at);
                let gain = n.velocity.clamp(0.0, 1.0).powf(1.5) * s.volume * VOICE_LEVEL;
                let (unit, end) = if n.channel == KIT_CHANNEL {
                    let p = piece(n.pitch).unwrap_or_else(|| {
                        unmapped.insert(n.pitch);
                        Piece::Stick
                    });
                    let dur = (score.seconds_at(n.at + n.dur) - start) as f32;
                    let (unit, length) = kit_voice(p, gain * s.expression, s.pan, dur);
                    (unit, start + length as f64)
                } else {
                    let written_end = score.seconds_at(n.at + n.dur);
                    match voice_for(s.patch) {
                        Voice::Melodic(r) => {
                            let end = extend_under_pedal(&sustain[channel], written_end);
                            let curve =
                                expression_curve(&lanes[channel], start, end + r.release as f64);
                            let unit = melodic_voice(
                                r,
                                hz(n.pitch) * bend_ratio(s.bend),
                                (end - start) as f32,
                                gain,
                                s.pan,
                                curve,
                            );
                            (unit, end + r.release as f64)
                        }
                        Voice::Texture(kind) => {
                            let curve = expression_curve(&lanes[channel], start, written_end);
                            let unit = texture_voice(
                                kind,
                                note_seed(channel, n),
                                n.pitch,
                                (written_end - start) as f32,
                                gain,
                                s.pan,
                                curve,
                            );
                            (unit, written_end + 0.3)
                        }
                    }
                };
                let unit = with_send(unit, s.reverb);
                let length = end - start;
                if length <= 0.0 {
                    continue;
                }
                seq.push(
                    start,
                    end,
                    Fade::Smooth,
                    0.001f64.min(length),
                    0.005f64.min(length),
                    unit,
                );
                last_end = last_end.max(end);
                voices += 1;
            }
        }
    }

    // The master: the dry pair through the room (a touch of short reverb on
    // everything), the send pair through the DISTANCE bus (air absorption, then
    // a long dark reverb), summed, then the limiter.
    let room = multipass::<U2>() & (reverb_stereo(10.0, 1.2, 0.6) * REVERB_WET);
    let distance = (lowpass_hz(DISTANCE_CUTOFF, 0.5) | lowpass_hz(DISTANCE_CUTOFF, 0.5))
        >> reverb_stereo(30.0, 3.0, 0.8);
    let left = (pass() | sink() | pass() | sink()) >> (join::<U2>() * 2.0);
    let right = (sink() | pass() | sink() | pass()) >> (join::<U2>() * 2.0);
    let buses = (room | distance) >> (left ^ right);
    let mut master = Net::wrap(Box::new(seq))
        >> Net::wrap(Box::new(buses))
        >> Net::wrap(Box::new(limiter_stereo(0.005, 0.1)));
    master.set_sample_rate(sample_rate);

    let warnings = unmapped
        .into_iter()
        .map(|n| format!("kit piece {n} has no voice; played the stick"))
        .collect();

    Built {
        master,
        duration_secs: last_end + TAIL_SECS,
        warnings,
        voices,
    }
}

/// Per channel, the spans (in seconds) during which the sustain pedal is down.
fn sustain_intervals(score: &Score) -> Vec<Vec<(f64, f64)>> {
    let mut spans: Vec<Vec<(f64, f64)>> = vec![Vec::new(); 16];
    let mut down: [Option<f64>; 16] = [None; 16];
    for event in &score.events {
        if let Event::Control(c) = event {
            if c.kind != ControlKind::Sustain {
                continue;
            }
            let ch = (c.channel & 0x0F) as usize;
            let t = score.seconds_at(c.at);
            match (c.value >= 0.5, down[ch]) {
                (true, None) => down[ch] = Some(t),
                (false, Some(from)) => {
                    spans[ch].push((from, t));
                    down[ch] = None;
                }
                _ => {}
            }
        }
    }
    let end = score.duration_secs();
    for (ch, from) in down.iter().enumerate() {
        if let Some(from) = from {
            spans[ch].push((*from, end.max(*from)));
        }
    }
    spans
}

/// Split a stereo voice into its dry pair and a send pair scaled by `send`.
fn with_send(unit: Box<dyn AudioUnit>, send: f32) -> Box<dyn AudioUnit> {
    let dry = Net::wrap(Box::new(multipass::<U2>()));
    let wet = Net::wrap(Box::new(multipass::<U2>() * send.clamp(0.0, 1.0)));
    Box::new(Net::wrap(unit) >> (dry ^ wet))
}

/// A texture individual's seed: where and what it is, so the same rows always
/// produce the same creature.
fn note_seed(channel: usize, n: &Note) -> u64 {
    (channel as u64)
        ^ ((n.pitch as u64) << 8)
        ^ ((n.at.num() as u64) << 16)
        ^ ((n.at.den() as u64) << 48)
}

/// Per channel, the expression lane as (seconds, value) breakpoints in time order.
fn expression_lanes(score: &Score) -> Vec<Vec<(f64, f32)>> {
    let mut lanes: Vec<Vec<(f64, f32)>> = vec![Vec::new(); 16];
    for event in &score.events {
        if let Event::Control(c) = event {
            if c.kind == ControlKind::Expression {
                lanes[(c.channel & 0x0F) as usize].push((score.seconds_at(c.at), c.value));
            }
        }
    }
    lanes
}

/// The expression curve over one note, as (seconds from the note's start,
/// value): the value in force at the start, every breakpoint inside, and the
/// value at the end. A lane with no rows is a flat 1.0.
fn expression_curve(lane: &[(f64, f32)], start: f64, end: f64) -> Vec<(f32, f32)> {
    let value_at = |t: f64| -> f32 {
        match lane.iter().rposition(|(at, _)| *at <= t) {
            None => lane.first().map_or(1.0, |(_, v)| *v),
            Some(i) => match lane.get(i + 1) {
                Some((next_at, next_v)) if *next_at > lane[i].0 => {
                    let f = ((t - lane[i].0) / (next_at - lane[i].0)).clamp(0.0, 1.0) as f32;
                    lane[i].1 + (next_v - lane[i].1) * f
                }
                _ => lane[i].1,
            },
        }
    };
    let mut curve = vec![(0.0f32, value_at(start))];
    for (at, v) in lane.iter().filter(|(at, _)| *at > start && *at < end) {
        curve.push(((at - start) as f32, *v));
    }
    curve.push(((end - start) as f32, value_at(end)));
    curve
}

/// Linear interpolation along an expression curve.
fn curve_at(curve: &[(f32, f32)], t: f32) -> f32 {
    match curve.iter().rposition(|(at, _)| *at <= t) {
        None => curve.first().map_or(1.0, |(_, v)| *v),
        Some(i) => match curve.get(i + 1) {
            Some((next_at, next_v)) if *next_at > curve[i].0 => {
                let f = ((t - curve[i].0) / (next_at - curve[i].0)).clamp(0.0, 1.0);
                curve[i].1 + (next_v - curve[i].1) * f
            }
            _ => curve[i].1,
        },
    }
}

/// A note whose written end falls while the pedal is down rings until it lifts.
fn extend_under_pedal(spans: &[(f64, f64)], end: f64) -> f64 {
    spans
        .iter()
        .find(|(from, to)| *from <= end && end < *to)
        .map_or(end, |(_, to)| *to)
}

/// Attack-decay-sustain-release as a pure function of time, for a note held
/// `hold` seconds. Release starts from wherever the curve was at `hold`.
pub fn adsr_at(t: f32, a: f32, d: f32, s: f32, r: f32, hold: f32) -> f32 {
    let (a, d, r) = (a.max(1e-3), d.max(1e-3), r.max(1e-3));
    let gate = |t: f32| {
        if t < a {
            t / a
        } else if t < a + d {
            1.0 + (s - 1.0) * ((t - a) / d)
        } else {
            s
        }
    };
    if t < hold {
        gate(t)
    } else {
        gate(hold) * (1.0 - (t - hold) / r).max(0.0)
    }
}

fn oscillator(osc: Osc, freq: f32) -> Net {
    match osc {
        Osc::Sine => Net::wrap(Box::new(sine_hz(freq))),
        Osc::Triangle => Net::wrap(Box::new(triangle_hz(freq))),
        Osc::Saw => Net::wrap(Box::new(saw_hz(freq))),
        Osc::SoftSaw => Net::wrap(Box::new(soft_saw_hz(freq))),
        Osc::Square => Net::wrap(Box::new(square_hz(freq))),
        Osc::Pulse(width) => Net::wrap(Box::new((dc(freq) | dc(width)) >> pulse())),
        Osc::Organ => Net::wrap(Box::new(organ_hz(freq))),
        Osc::Hammond => Net::wrap(Box::new(hammond_hz(freq))),
        Osc::Struck => Net::wrap(Box::new(
            lfo(move |t: f32| freq * (1.0 + 0.6 * (-t * 25.0).exp())) >> sine(),
        )),
    }
}

/// The cutoff a recipe actually gets at `freq`: its own, capped at
/// [`MAX_HARMONICS`] above the note. Zero stays zero (no filter).
pub fn tracked_cutoff(cutoff: f32, freq: f32) -> f32 {
    if cutoff <= 0.0 {
        0.0
    } else {
        cutoff.min(freq * MAX_HARMONICS)
    }
}

/// One melodic voice: oscillator → lowpass → ADSR × expression curve → level → pan.
fn melodic_voice(
    r: Recipe,
    freq: f32,
    hold: f32,
    gain: f32,
    pan_pos: f32,
    expression: Vec<(f32, f32)>,
) -> Box<dyn AudioUnit> {
    let mut net = oscillator(r.osc, freq);
    let cutoff = tracked_cutoff(r.cutoff, freq);
    if cutoff > 0.0 {
        net = net >> Net::wrap(Box::new(lowpass_hz(cutoff, 0.7)));
    }
    let (a, d, s, rel) = (r.attack, r.decay, r.sustain, r.release);
    let envelope = Net::wrap(Box::new(lfo(move |t: f32| {
        adsr_at(t, a, d, s, rel, hold) * curve_at(&expression, t)
    })));
    Box::new((net * envelope * (gain * r.level)) >> Net::wrap(Box::new(pan(pan_pos))))
}

/// One kit hit: a body, a decay, a level, a pan. Returns the voice and its length in seconds.
fn kit_voice(p: Piece, gain: f32, pan_pos: f32, dur: f32) -> (Box<dyn AudioUnit>, f32) {
    let decay = |k: f32| Net::wrap(Box::new(lfo(move |t: f32| (-t * k).exp())));
    let hiss = |cutoff: f32| Net::wrap(Box::new(white() >> highpass_hz(cutoff, 0.6)));
    let (body, length): (Net, f32) = match p {
        Piece::Kick => (
            Net::wrap(Box::new(
                lfo(|t: f32| 48.0 + 160.0 * (-t * 30.0).exp()) >> sine(),
            )) * decay(9.0)
                * 1.4,
            0.6,
        ),
        Piece::Snare => (
            hiss(900.0) * decay(18.0) * 0.8
                + Net::wrap(Box::new(sine_hz(190.0))) * decay(25.0) * 0.6,
            0.35,
        ),
        Piece::Stick => (hiss(1500.0) * decay(40.0) * 0.6, 0.15),
        Piece::Clap => (hiss(1200.0) * decay(22.0) * 0.8, 0.25),
        Piece::ClosedHat => (hiss(7000.0) * decay(45.0) * 0.5, 0.12),
        Piece::OpenHat => (hiss(6000.0) * decay(7.0) * 0.45, 0.6),
        Piece::Tom(f) => (
            Net::wrap(Box::new(
                lfo(move |t: f32| f + 50.0 * (-t * 15.0).exp()) >> sine(),
            )) * decay(8.0)
                * 1.1,
            0.55,
        ),
        Piece::Crash => (hiss(3500.0) * decay(2.2) * 0.5, 1.8),
        Piece::Ride => (hiss(5000.0) * decay(4.0) * 0.4, 1.0),
        Piece::SnareRoll => {
            // A buzz roll: rapid strokes for the note's length, swelling, then a decay.
            let roll = dur.max(0.1);
            let env = Net::wrap(Box::new(lfo(move |t: f32| {
                if t < roll {
                    let stroke = (0.5 + 0.5 * (std::f32::consts::TAU * 28.0 * t).sin()).powf(3.0);
                    stroke * (0.35 + 0.65 * t / roll)
                } else {
                    0.8 * (-(t - roll) * 20.0).exp()
                }
            })));
            (hiss(900.0) * env * 0.8, roll + 0.2)
        }
    };
    (
        Box::new((body * gain) >> Net::wrap(Box::new(pan(pan_pos)))),
        length,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::patch::recipe;
    use crate::smf;

    #[test]
    fn the_envelope_attacks_decays_holds_and_releases() {
        let (a, d, s, r) = (0.1, 0.2, 0.5, 0.4);
        assert_eq!(adsr_at(0.0, a, d, s, r, 1.0), 0.0);
        assert!((adsr_at(0.05, a, d, s, r, 1.0) - 0.5).abs() < 1e-6);
        assert!((adsr_at(0.1, a, d, s, r, 1.0) - 1.0).abs() < 1e-6);
        assert!((adsr_at(0.2, a, d, s, r, 1.0) - 0.75).abs() < 1e-6);
        assert!((adsr_at(0.5, a, d, s, r, 1.0) - 0.5).abs() < 1e-6);
        // Release from the held level, reaching zero after `r`.
        assert!((adsr_at(1.2, a, d, s, r, 1.0) - 0.25).abs() < 1e-6);
        assert_eq!(adsr_at(1.5, a, d, s, r, 1.0), 0.0);
        // A note shorter than its attack releases from the level it reached.
        assert!((adsr_at(0.05, a, d, s, r, 0.05) - 0.5).abs() < 1e-6);
        assert!((adsr_at(0.25, a, d, s, r, 0.05) - 0.25).abs() < 1e-6);
    }

    #[test]
    fn the_cutoff_follows_the_note_down() {
        // A 29 Hz bass note on a 6 kHz recipe closes to 24 harmonics.
        assert!((tracked_cutoff(6000.0, 29.0) - 696.0).abs() < 1e-3);
        // A 440 Hz note keeps the recipe's own cutoff.
        assert_eq!(tracked_cutoff(6000.0, 440.0), 6000.0);
        // No filter stays no filter.
        assert_eq!(tracked_cutoff(0.0, 29.0), 0.0);
    }

    #[test]
    fn the_reverb_send_puts_a_tail_on_a_short_note() {
        use crate::score::{Control, ControlKind, Note, Program};
        use crate::Pos;
        let piece = |send: f32| {
            let mut score = Score::default();
            score.events.push(Event::Program(Program {
                at: Pos::ZERO,
                channel: 0,
                patch: 10,
            }));
            score.events.push(Event::Control(Control {
                at: Pos::ZERO,
                channel: 0,
                kind: ControlKind::Reverb,
                value: send,
            }));
            score.events.push(Event::Note(Note {
                at: Pos::ZERO,
                dur: Pos::new(1, 8),
                channel: 0,
                pitch: 72,
                velocity: 1.0,
            }));
            score.end = Pos::new(2, 1);
            score.sort();
            crate::render::render(&score, 8_000.0)
        };
        let dry = piece(0.0);
        let far = piece(1.0);
        let tail = |r: &crate::render::Rendered| {
            let ch = r.wave.channel(0);
            let (a, b) = (2 * 8_000, 3 * 8_000);
            (ch[a..b].iter().map(|x| x * x).sum::<f32>() / 8_000.0).sqrt()
        };
        assert!(
            tail(&far) > 3.0 * tail(&dry),
            "no distance tail: dry {:.5} far {:.5}",
            tail(&dry),
            tail(&far)
        );
        assert_eq!(far.wave.channels(), 2);
    }

    #[test]
    fn the_expression_lane_shapes_a_held_note() {
        use crate::score::{Control, ControlKind, Note, Program};
        use crate::Pos;
        // One four-bar flute note at 120 bpm (8 s) under a swell: 0 → 1 → 0.
        let mut score = Score::default();
        score.events.push(Event::Program(Program {
            at: Pos::ZERO,
            channel: 0,
            patch: 73,
        }));
        for (at, value) in [
            (Pos::ZERO, 0.0),
            (Pos::new(2, 1), 1.0),
            (Pos::new(4, 1), 0.0),
        ] {
            score.events.push(Event::Control(Control {
                at,
                channel: 0,
                kind: ControlKind::Expression,
                value,
            }));
        }
        score.events.push(Event::Note(Note {
            at: Pos::ZERO,
            dur: Pos::new(4, 1),
            channel: 0,
            pitch: 69,
            velocity: 1.0,
        }));
        score.end = Pos::new(4, 1);
        score.sort();
        let sr = 8_000.0;
        let r = crate::render::render(&score, sr);
        let rms = |from: f64, to: f64| {
            let (a, b) = ((from * sr) as usize, (to * sr) as usize);
            (r.wave.channel(0)[a..b].iter().map(|x| x * x).sum::<f32>() / (b - a) as f32).sqrt()
        };
        let (early, middle, late) = (rms(0.5, 1.0), rms(3.75, 4.25), rms(7.0, 7.5));
        assert!(
            middle > 3.0 * early,
            "no swell: early {early:.4} middle {middle:.4}"
        );
        assert!(
            middle > 3.0 * late,
            "no fade: middle {middle:.4} late {late:.4}"
        );
        // The curve helper itself: flat lanes stay flat, breakpoints interpolate.
        assert_eq!(curve_at(&[(0.0, 1.0)], 5.0), 1.0);
        assert!((curve_at(&[(0.0, 0.0), (4.0, 1.0)], 2.0) - 0.5).abs() < 1e-6);
        assert_eq!(
            expression_curve(&[], 1.0, 3.0),
            vec![(0.0, 1.0), (2.0, 1.0)]
        );
    }

    #[test]
    fn sustain_spans_extend_notes_that_end_under_the_pedal() {
        let score = smf::read(&smf::fixture::score_bytes()).unwrap();
        let spans = sustain_intervals(&score);
        // Pedal down at 1/2 (1.0 s), up at 3/4 (1.5 s) on channel 1.
        assert_eq!(spans[0], vec![(1.0, 1.5)]);
        assert!(spans[9].is_empty());
        // The eighth written to end at 1.25 s rings until the pedal lifts.
        assert_eq!(extend_under_pedal(&spans[0], 1.25), 1.5);
        // A note ending after the lift, or before the press, is untouched.
        assert_eq!(extend_under_pedal(&spans[0], 1.5), 1.5);
        assert_eq!(extend_under_pedal(&spans[0], 0.75), 0.75);
    }

    #[test]
    fn builds_every_note_and_reports_the_unmapped_piece() {
        let score = smf::read(&smf::fixture::score_bytes()).unwrap();
        let built = build(&score, 22_050.0);
        assert_eq!(built.voices, 10);
        assert_eq!(built.master.inputs(), 0);
        assert_eq!(built.master.outputs(), 2);
        assert_eq!(
            built.warnings,
            vec!["kit piece 75 has no voice; played the stick"]
        );
        // The conductor track's end-of-track (bar 3) outlasts every release, so
        // the piece ends where the file says it ends, plus the tail.
        assert!((built.duration_secs - (score.duration_secs() + TAIL_SECS)).abs() < 1e-6);
    }

    #[test]
    fn a_release_past_the_written_end_extends_the_piece() {
        use crate::score::{Note, Program};
        use crate::Pos;
        let mut score = Score::default();
        score.events.push(Event::Program(Program {
            at: Pos::ZERO,
            channel: 0,
            patch: 88,
        }));
        score.events.push(Event::Note(Note {
            at: Pos::ZERO,
            dur: Pos::new(1, 4),
            channel: 0,
            pitch: 60,
            velocity: 1.0,
        }));
        score.end = Pos::new(1, 4);
        let built = build(&score, 22_050.0);
        let release = recipe(88).release as f64;
        assert!((built.duration_secs - (0.5 + release + TAIL_SECS)).abs() < 1e-6);
        assert_eq!(built.voices, 1);
    }
}
