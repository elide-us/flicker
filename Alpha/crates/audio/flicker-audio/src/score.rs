//! The event ROWS — the contract between whatever produces music (today the
//! MIDI reader, later the generator) and whatever consumes it (the synth, a
//! visual, an exporter).
//!
//! Every row carries an exact [`Pos`]. A note is ONE row with a duration, not
//! an on/off pair. Pitch is ONE number, the absolute pitch index (69 = A4);
//! the synth resolves it to hertz through the tuning table. Controller values
//! are plain floats, not 7-bit integers.

use crate::time::{seconds_at, Pos, TempoChange};

/// One sounding note. `velocity` is 0..1.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Note {
    pub at: Pos,
    pub dur: Pos,
    pub channel: u8,
    /// Absolute pitch index: 60 = middle C, 69 = A4 = 440 Hz under 12-TET.
    /// On the kit channel it is the piece index instead.
    pub pitch: u8,
    pub velocity: f32,
}

/// A continuous-controller lane.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ControlKind {
    /// Channel level, 0..1.
    Volume,
    /// Performance swell on top of volume, 0..1.
    Expression,
    /// Stereo position, -1 (left) .. 1 (right).
    Pan,
    /// Sustain pedal: 1 held, 0 released. Held notes keep ringing past their
    /// written end until the pedal lifts.
    Sustain,
    /// Pitch bend in semitones (a bend range of ±2 is assumed at the reader).
    PitchBend,
    /// Send into the DISTANCE bus (a long, dark reverb), 0..1. MIDI's CC91.
    /// Zero is the engine default; far-off layers sit high here.
    Reverb,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Control {
    pub at: Pos,
    pub channel: u8,
    pub kind: ControlKind,
    pub value: f32,
}

/// Bind a patch (instrument recipe) to a channel from `at` onward.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Program {
    pub at: Pos,
    pub channel: u8,
    /// Patch index — today the General MIDI program number, 0..127.
    pub patch: u8,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Event {
    Note(Note),
    Control(Control),
    Program(Program),
}

impl Event {
    pub fn at(&self) -> Pos {
        match self {
            Event::Note(n) => n.at,
            Event::Control(c) => c.at,
            Event::Program(p) => p.at,
        }
    }

    pub fn channel(&self) -> u8 {
        match self {
            Event::Note(n) => n.channel,
            Event::Control(c) => c.channel,
            Event::Program(p) => p.channel,
        }
    }

    /// Order within one instant: a patch binding and controller values take
    /// effect before a note that shares their position.
    fn rank(&self) -> u8 {
        match self {
            Event::Program(_) => 0,
            Event::Control(_) => 1,
            Event::Note(_) => 2,
        }
    }
}

/// A meter row: `numerator / denominator` from `at` onward (4/4, 6/8 …).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Meter {
    pub at: Pos,
    pub numerator: u8,
    pub denominator: u8,
}

/// The rows of one piece. `events` is kept sorted by position (see [`Score::sort`]).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Score {
    pub events: Vec<Event>,
    /// The tempo map, sorted by `at`. Empty means the default tempo throughout.
    pub tempo: Vec<TempoChange>,
    pub meter: Vec<Meter>,
    /// Where the piece ends (the latest end-of-track or note end).
    pub end: Pos,
}

impl Score {
    /// Seconds from the origin to `pos` under this score's tempo rows.
    pub fn seconds_at(&self, pos: Pos) -> f64 {
        seconds_at(&self.tempo, pos)
    }

    /// Length of the piece in seconds (to `end`, excluding any release tail).
    pub fn duration_secs(&self) -> f64 {
        self.seconds_at(self.end)
    }

    /// Stable sort: by position, then bindings → controls → notes.
    pub fn sort(&mut self) {
        self.events
            .sort_by(|a, b| a.at().cmp(&b.at()).then(a.rank().cmp(&b.rank())));
        self.tempo.sort_by_key(|t| t.at);
        self.meter.sort_by_key(|m| m.at);
    }

    pub fn notes(&self) -> impl Iterator<Item = &Note> {
        self.events.iter().filter_map(|e| match e {
            Event::Note(n) => Some(n),
            _ => None,
        })
    }
}
