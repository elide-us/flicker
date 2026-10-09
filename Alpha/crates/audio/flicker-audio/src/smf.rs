//! Standard MIDI File reader — the ONE place MIDI's byte format exists in the
//! engine. A file is read once into [`Score`] rows; nothing downstream knows
//! MIDI. Formats 0 and 1 with a pulses-per-quarter division are read; SMPTE
//! division and format 2 fail loud rather than play wrong.

use std::collections::HashMap;

use crate::score::{Control, ControlKind, Event, Meter, Note, Program, Score};
use crate::time::{Pos, TempoChange};

#[derive(Debug, thiserror::Error)]
pub enum SmfError {
    #[error("not a Standard MIDI File (no MThd header chunk)")]
    NotSmf,
    #[error("file ends early at byte {0}")]
    Truncated(usize),
    #[error("SMF format {0} is not supported (formats 0 and 1 are)")]
    UnsupportedFormat(u16),
    #[error("SMPTE time division is not supported (pulses per quarter only)")]
    SmpteDivision,
    #[error("zero pulses per quarter note")]
    BadDivision,
    #[error("status byte {0:#04x} at byte {1} is not valid in a file")]
    BadStatus(u8, usize),
}

/// What the header chunk said — for the tester's summary, not for playback.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Header {
    pub format: u16,
    pub tracks: u16,
    pub ppq: u16,
}

/// Read a whole file's bytes into rows.
pub fn read(bytes: &[u8]) -> Result<Score, SmfError> {
    read_with_header(bytes).map(|(_, score)| score)
}

/// [`read`], also returning the header facts.
pub fn read_with_header(bytes: &[u8]) -> Result<(Header, Score), SmfError> {
    let mut cur = Cursor { bytes, pos: 0 };
    let (id, body) = cur.chunk().map_err(|_| SmfError::NotSmf)?;
    if id != *b"MThd" || body.len() < 6 {
        return Err(SmfError::NotSmf);
    }
    let format = be16(&body[0..2]);
    let tracks = be16(&body[2..4]);
    let division = be16(&body[4..6]);
    if format > 1 {
        return Err(SmfError::UnsupportedFormat(format));
    }
    if division & 0x8000 != 0 {
        return Err(SmfError::SmpteDivision);
    }
    if division == 0 {
        return Err(SmfError::BadDivision);
    }
    let ppq = division;

    let mut score = Score::default();
    while cur.remaining() > 0 {
        let (id, body) = cur.chunk()?;
        if id != *b"MTrk" {
            // The spec asks readers to skip chunk types they do not know.
            continue;
        }
        read_track(body, ppq, &mut score)?;
    }
    score.sort();
    Ok((
        Header {
            format,
            tracks,
            ppq,
        },
        score,
    ))
}

fn be16(b: &[u8]) -> u16 {
    u16::from_be_bytes([b[0], b[1]])
}

struct Cursor<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    fn remaining(&self) -> usize {
        self.bytes.len() - self.pos
    }

    fn u8(&mut self) -> Result<u8, SmfError> {
        let b = *self
            .bytes
            .get(self.pos)
            .ok_or(SmfError::Truncated(self.pos))?;
        self.pos += 1;
        Ok(b)
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8], SmfError> {
        let end = self
            .pos
            .checked_add(n)
            .filter(|&end| end <= self.bytes.len())
            .ok_or(SmfError::Truncated(self.bytes.len()))?;
        let out = &self.bytes[self.pos..end];
        self.pos = end;
        Ok(out)
    }

    /// Variable-length quantity: 7 bits per byte, high bit = more follows, max 4 bytes.
    fn vlq(&mut self) -> Result<u32, SmfError> {
        let mut value: u32 = 0;
        for _ in 0..4 {
            let b = self.u8()?;
            value = (value << 7) | (b & 0x7F) as u32;
            if b & 0x80 == 0 {
                return Ok(value);
            }
        }
        Err(SmfError::BadStatus(0x80, self.pos))
    }

    fn chunk(&mut self) -> Result<([u8; 4], &'a [u8]), SmfError> {
        let id = self.take(4)?;
        let len = u32::from_be_bytes(self.take(4)?.try_into().expect("four bytes")) as usize;
        let body = self.take(len)?;
        Ok(([id[0], id[1], id[2], id[3]], body))
    }
}

type Open = HashMap<(u8, u8), (u64, f32)>;

fn close_note(
    score: &mut Score,
    open: &mut Open,
    channel: u8,
    pitch: u8,
    end_ticks: u64,
    ppq: u16,
) {
    if let Some((start, velocity)) = open.remove(&(channel, pitch)) {
        // A zero-length note has no sound to make; drop it rather than push a
        // degenerate row.
        if end_ticks > start {
            score.events.push(Event::Note(Note {
                at: Pos::from_ticks(start, ppq),
                dur: Pos::from_ticks(end_ticks - start, ppq),
                channel,
                pitch,
                velocity,
            }));
        }
    }
}

fn read_track(body: &[u8], ppq: u16, score: &mut Score) -> Result<(), SmfError> {
    let mut c = Cursor {
        bytes: body,
        pos: 0,
    };
    let mut ticks: u64 = 0;
    // Running status: a channel message may omit its status byte and reuse the
    // previous one. Strictly, meta and sysex events cancel it; files in the
    // wild rely on it surviving, and a compliant file is unaffected by leniency.
    let mut running: Option<u8> = None;
    let mut open: Open = HashMap::new();
    let mut ended = false;

    while c.remaining() > 0 && !ended {
        ticks += c.vlq()? as u64;
        let at = Pos::from_ticks(ticks, ppq);
        let first = c.u8()?;
        let status = if first & 0x80 != 0 {
            first
        } else {
            c.pos -= 1;
            running.ok_or(SmfError::BadStatus(first, c.pos))?
        };
        match status {
            0xFF => {
                let kind = c.u8()?;
                let len = c.vlq()? as usize;
                let data = c.take(len)?;
                match kind {
                    0x51 if len == 3 => {
                        let micros = u32::from_be_bytes([0, data[0], data[1], data[2]]);
                        score
                            .tempo
                            .push(TempoChange::from_micros_per_quarter(at, micros));
                    }
                    0x58 if len >= 2 => score.meter.push(Meter {
                        at,
                        numerator: data[0],
                        denominator: 1u8 << data[1].min(7),
                    }),
                    0x2F => ended = true,
                    _ => {}
                }
            }
            0xF0 | 0xF7 => {
                let len = c.vlq()? as usize;
                c.take(len)?;
            }
            0xF1..=0xF6 | 0xF8..=0xFE => return Err(SmfError::BadStatus(status, c.pos - 1)),
            _ => {
                running = Some(status);
                let channel = status & 0x0F;
                match status & 0xF0 {
                    0x80 => {
                        let pitch = c.u8()? & 0x7F;
                        let _release_velocity = c.u8()?;
                        close_note(score, &mut open, channel, pitch, ticks, ppq);
                    }
                    0x90 => {
                        let pitch = c.u8()? & 0x7F;
                        let velocity = c.u8()? & 0x7F;
                        if velocity == 0 {
                            close_note(score, &mut open, channel, pitch, ticks, ppq);
                        } else {
                            // A retrigger closes the note already sounding.
                            close_note(score, &mut open, channel, pitch, ticks, ppq);
                            open.insert((channel, pitch), (ticks, velocity as f32 / 127.0));
                        }
                    }
                    0xA0 => {
                        c.take(2)?;
                    }
                    0xB0 => {
                        let controller = c.u8()? & 0x7F;
                        let value = (c.u8()? & 0x7F) as f32;
                        let lane = match controller {
                            7 => Some((ControlKind::Volume, value / 127.0)),
                            10 => {
                                Some((ControlKind::Pan, ((value - 64.0) / 64.0).clamp(-1.0, 1.0)))
                            }
                            11 => Some((ControlKind::Expression, value / 127.0)),
                            64 => {
                                Some((ControlKind::Sustain, if value >= 64.0 { 1.0 } else { 0.0 }))
                            }
                            91 => Some((ControlKind::Reverb, value / 127.0)),
                            _ => None,
                        };
                        if let Some((kind, value)) = lane {
                            score.events.push(Event::Control(Control {
                                at,
                                channel,
                                kind,
                                value,
                            }));
                        }
                    }
                    0xC0 => {
                        let patch = c.u8()? & 0x7F;
                        score
                            .events
                            .push(Event::Program(Program { at, channel, patch }));
                    }
                    0xD0 => {
                        c.take(1)?;
                    }
                    0xE0 => {
                        let lsb = (c.u8()? & 0x7F) as i32;
                        let msb = (c.u8()? & 0x7F) as i32;
                        let raw = ((msb << 7) | lsb) - 8192;
                        score.events.push(Event::Control(Control {
                            at,
                            channel,
                            kind: ControlKind::PitchBend,
                            value: raw as f32 / 8192.0 * 2.0,
                        }));
                    }
                    _ => unreachable!("system statuses are matched above"),
                }
            }
        }
    }

    // Notes still sounding when the track ends close at the track's end.
    let keys: Vec<(u8, u8)> = open.keys().copied().collect();
    for (channel, pitch) in keys {
        close_note(score, &mut open, channel, pitch, ticks, ppq);
    }
    let track_end = Pos::from_ticks(ticks, ppq);
    if track_end > score.end {
        score.end = track_end;
    }
    Ok(())
}

/// Hand-built files for the tests: the slice-1 fixture and the byte helpers
/// behind it. Shared with the synth and render tests.
#[cfg(test)]
pub(crate) mod fixture {
    pub fn vlq(mut v: u32) -> Vec<u8> {
        let mut out = vec![(v & 0x7F) as u8];
        v >>= 7;
        while v > 0 {
            out.insert(0, 0x80 | (v & 0x7F) as u8);
            v >>= 7;
        }
        out
    }

    pub fn chunk(id: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let mut out = id.to_vec();
        out.extend_from_slice(&(body.len() as u32).to_be_bytes());
        out.extend_from_slice(body);
        out
    }

    pub fn header(format: u16, tracks: u16, division: u16) -> Vec<u8> {
        let mut body = Vec::new();
        body.extend_from_slice(&format.to_be_bytes());
        body.extend_from_slice(&tracks.to_be_bytes());
        body.extend_from_slice(&division.to_be_bytes());
        chunk(b"MThd", &body)
    }

    pub fn ev(delta: u32, bytes: &[u8]) -> Vec<u8> {
        let mut out = vlq(delta);
        out.extend_from_slice(bytes);
        out
    }

    pub fn tempo(delta: u32, micros: u32) -> Vec<u8> {
        let b = micros.to_be_bytes();
        ev(delta, &[0xFF, 0x51, 0x03, b[1], b[2], b[3]])
    }

    pub fn time_sig(delta: u32, numerator: u8, denominator_pow2: u8) -> Vec<u8> {
        ev(
            delta,
            &[0xFF, 0x58, 0x04, numerator, denominator_pow2, 24, 8],
        )
    }

    pub fn end_of_track(delta: u32) -> Vec<u8> {
        ev(delta, &[0xFF, 0x2F, 0x00])
    }

    pub fn track(events: &[Vec<u8>]) -> Vec<u8> {
        chunk(b"MTrk", &events.concat())
    }

    /// Format 1, 480 ppq, three tracks. Bar 1 at 120 bpm, bar 2 at 90 bpm, 4/4.
    /// Channel 1 (square lead): a quarter, three triplet eighths via running
    /// status, a sustained eighth under the pedal, then a bent, hard-left quarter.
    /// Channel 10 (kit): kick, snare, closed hat and one UNMAPPED piece (claves).
    /// A sysex sits in the lead track to prove it is skipped.
    pub fn score_bytes() -> Vec<u8> {
        let conductor = track(&[
            tempo(0, 500_000),
            time_sig(0, 4, 2),
            tempo(1920, 666_667),
            end_of_track(1920),
        ]);
        let lead = track(&[
            ev(0, &[0xC0, 80]),
            ev(0, &[0xB0, 7, 100]),
            ev(0, &[0xF0, 0x05, 0x7E, 0x7F, 0x09, 0x01, 0xF7]),
            ev(0, &[0x90, 60, 100]),
            ev(480, &[60, 0]),
            ev(0, &[64, 96]),
            ev(160, &[64, 0]),
            ev(0, &[67, 96]),
            ev(160, &[67, 0]),
            ev(0, &[71, 96]),
            ev(160, &[71, 0]),
            ev(0, &[0xB0, 64, 127]),
            ev(0, &[0x90, 62, 80]),
            ev(240, &[62, 0]),
            ev(240, &[0xB0, 64, 0]),
            ev(0, &[0xE0, 0x00, 0x60]),
            ev(0, &[0xB0, 10, 0]),
            ev(0, &[0x90, 67, 64]),
            ev(480, &[67, 0]),
            end_of_track(0),
        ]);
        let kit = track(&[
            ev(0, &[0x99, 36, 127]),
            ev(60, &[36, 0]),
            ev(420, &[38, 112]),
            ev(60, &[38, 0]),
            ev(180, &[42, 96]),
            ev(60, &[42, 0]),
            ev(180, &[75, 96]),
            ev(60, &[75, 0]),
            end_of_track(900),
        ]);
        [header(1, 3, 480), conductor, lead, kit].concat()
    }
}

#[cfg(test)]
mod tests {
    use super::fixture::*;
    use super::*;

    fn notes_on(score: &Score, channel: u8) -> Vec<(Pos, Pos, u8)> {
        score
            .notes()
            .filter(|n| n.channel == channel)
            .map(|n| (n.at, n.dur, n.pitch))
            .collect()
    }

    #[test]
    fn reads_the_fixture_into_exact_rows() {
        let (header, score) = read_with_header(&score_bytes()).unwrap();
        assert_eq!(
            header,
            Header {
                format: 1,
                tracks: 3,
                ppq: 480
            }
        );
        assert_eq!(
            notes_on(&score, 0),
            vec![
                (Pos::ZERO, Pos::new(1, 4), 60),
                (Pos::new(1, 4), Pos::new(1, 12), 64),
                (Pos::new(1, 3), Pos::new(1, 12), 67),
                (Pos::new(5, 12), Pos::new(1, 12), 71),
                (Pos::new(1, 2), Pos::new(1, 8), 62),
                (Pos::new(3, 4), Pos::new(1, 4), 67),
            ]
        );
        assert_eq!(notes_on(&score, 9).len(), 4);
        assert_eq!(notes_on(&score, 9)[0], (Pos::ZERO, Pos::new(1, 32), 36));

        let controls: Vec<(Pos, ControlKind, f32)> = score
            .events
            .iter()
            .filter_map(|e| match e {
                Event::Control(c) if c.channel == 0 => Some((c.at, c.kind, c.value)),
                _ => None,
            })
            .collect();
        assert_eq!(controls[0], (Pos::ZERO, ControlKind::Volume, 100.0 / 127.0));
        assert_eq!(controls[1], (Pos::new(1, 2), ControlKind::Sustain, 1.0));
        assert_eq!(controls[2], (Pos::new(3, 4), ControlKind::Sustain, 0.0));
        assert_eq!(controls[3], (Pos::new(3, 4), ControlKind::PitchBend, 1.0));
        assert_eq!(controls[4], (Pos::new(3, 4), ControlKind::Pan, -1.0));
        assert_eq!(controls.len(), 5);

        let programs: Vec<&Program> = score
            .events
            .iter()
            .filter_map(|e| match e {
                Event::Program(p) => Some(p),
                _ => None,
            })
            .collect();
        assert_eq!(
            programs,
            vec![&Program {
                at: Pos::ZERO,
                channel: 0,
                patch: 80
            }]
        );

        // Bindings and controls precede the notes they share an instant with.
        let at_half: Vec<u8> = score
            .events
            .iter()
            .filter(|e| e.at() == Pos::new(1, 2) && e.channel() == 0)
            .map(|e| match e {
                Event::Control(_) => 1,
                Event::Note(_) => 2,
                Event::Program(_) => 0,
            })
            .collect();
        assert_eq!(at_half, vec![1, 2]);

        assert_eq!(score.tempo.len(), 2);
        assert_eq!(score.tempo[0].at, Pos::ZERO);
        assert!((score.tempo[0].bpm() - 120.0).abs() < 1e-9);
        assert_eq!(score.tempo[1].at, Pos::new(1, 1));
        assert!((score.tempo[1].bpm() - 90.0).abs() < 1e-3);
        assert_eq!(
            score.meter,
            vec![Meter {
                at: Pos::ZERO,
                numerator: 4,
                denominator: 4
            }]
        );
        assert_eq!(score.end, Pos::new(2, 1));
        // Bar 1 at 120 bpm is 2 s; bar 2 at 90 bpm is 2.667 s.
        assert!((score.duration_secs() - (2.0 + 4.0 * 0.666_667)).abs() < 1e-6);
    }

    #[test]
    fn refuses_what_it_does_not_speak() {
        assert!(matches!(read(b"RIFF....WAVE"), Err(SmfError::NotSmf)));
        let format2 = [header(2, 0, 480)].concat();
        assert!(matches!(
            read(&format2),
            Err(SmfError::UnsupportedFormat(2))
        ));
        let smpte = [header(0, 1, 0xE728)].concat();
        assert!(matches!(read(&smpte), Err(SmfError::SmpteDivision)));
        let zero = [header(0, 1, 0)].concat();
        assert!(matches!(read(&zero), Err(SmfError::BadDivision)));
        let bytes = score_bytes();
        assert!(matches!(
            read(&bytes[..bytes.len() / 2]),
            Err(SmfError::Truncated(_))
        ));
        let orphan_data = [header(0, 1, 96), track(&[ev(0, &[60, 100])])].concat();
        assert!(matches!(
            read(&orphan_data),
            Err(SmfError::BadStatus(60, _))
        ));
    }

    #[test]
    fn the_reverb_send_controller_is_a_lane() {
        let bytes = [
            header(0, 1, 96),
            track(&[
                ev(0, &[0xB0, 91, 64]),
                ev(0, &[0x90, 60, 100]),
                ev(96, &[60, 0]),
                end_of_track(0),
            ]),
        ]
        .concat();
        let score = read(&bytes).unwrap();
        let sends: Vec<f32> = score
            .events
            .iter()
            .filter_map(|e| match e {
                Event::Control(c) if c.kind == ControlKind::Reverb => Some(c.value),
                _ => None,
            })
            .collect();
        assert_eq!(sends.len(), 1);
        assert!((sends[0] - 64.0 / 127.0).abs() < 1e-6);
    }

    #[test]
    fn unterminated_notes_close_at_the_track_end() {
        let bytes = [
            header(0, 1, 96),
            track(&[ev(0, &[0x90, 60, 100]), end_of_track(96)]),
        ]
        .concat();
        let score = read(&bytes).unwrap();
        assert_eq!(notes_on(&score, 0), vec![(Pos::ZERO, Pos::new(1, 4), 60)]);
        assert_eq!(score.end, Pos::new(1, 4));
    }

    #[test]
    fn zero_length_and_retriggered_notes() {
        let bytes = [
            header(0, 1, 96),
            track(&[
                ev(0, &[0x90, 60, 100]),
                ev(0, &[60, 0]),
                ev(0, &[0x90, 62, 100]),
                ev(48, &[62, 100]),
                ev(48, &[62, 0]),
                end_of_track(0),
            ]),
        ]
        .concat();
        let score = read(&bytes).unwrap();
        assert_eq!(
            notes_on(&score, 0),
            vec![
                (Pos::ZERO, Pos::new(1, 8), 62),
                (Pos::new(1, 8), Pos::new(1, 8), 62)
            ]
        );
    }
}
