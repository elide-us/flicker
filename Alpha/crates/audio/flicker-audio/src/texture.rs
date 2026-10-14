//! Soundscape TEXTURES: voices that are a procedural scene for the length of a
//! note rather than a pitched tone — crickets, frogs, a breeze. A texture note
//! is one INDIVIDUAL: its pitch chooses the species and its stereo place, its
//! seed fixes its schedule, so the same rows always chirp the same way. These
//! live beyond General MIDI's 128 programs as engine-native patch numbers.

use fundsp::prelude32::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Texture {
    Crickets,
    Frogs,
    Breeze,
}

/// Engine-native programs, past the General MIDI range.
pub const PROGRAM_CRICKETS: u8 = 128;
pub const PROGRAM_FROGS: u8 = 129;
pub const PROGRAM_BREEZE: u8 = 130;

impl Texture {
    pub fn from_program(program: u8) -> Option<Texture> {
        match program {
            PROGRAM_CRICKETS => Some(Texture::Crickets),
            PROGRAM_FROGS => Some(Texture::Frogs),
            PROGRAM_BREEZE => Some(Texture::Breeze),
            _ => None,
        }
    }

    pub fn program(self) -> u8 {
        match self {
            Texture::Crickets => PROGRAM_CRICKETS,
            Texture::Frogs => PROGRAM_FROGS,
            Texture::Breeze => PROGRAM_BREEZE,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Texture::Crickets => "Crickets",
            Texture::Frogs => "Frogs",
            Texture::Breeze => "Breeze",
        }
    }
}

/// A seeded number in 0..1 for index `i` — no state, so any sample can ask.
fn hash01(seed: u64, i: u64) -> f32 {
    let mut x = seed ^ i.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^= x >> 31;
    (x >> 40) as f32 / (1u64 << 24) as f32
}

/// Events (start, length) in seconds across `duration`: a seeded gap, then a
/// seeded length, repeated.
fn schedule(seed: u64, duration: f32, gap: (f32, f32), len: (f32, f32)) -> Vec<(f32, f32)> {
    let mut out = Vec::new();
    let mut t = gap.0 * hash01(seed, 0);
    let mut i = 1u64;
    while t < duration {
        let l = len.0 + (len.1 - len.0) * hash01(seed, i);
        out.push((t, l));
        t += l + gap.0 + (gap.1 - gap.0) * hash01(seed, i + 1);
        i += 2;
    }
    out
}

/// Position inside the event covering `t`, as (time into it, its length).
fn inside(events: &[(f32, f32)], t: f32) -> Option<(f32, f32)> {
    events
        .iter()
        .find(|(start, len)| t >= *start && t < start + len)
        .map(|(start, len)| (t - start, *len))
}

/// A soft gate: 1 inside an event, ramping over `ramp` seconds at both ends.
fn gate(events: &[(f32, f32)], t: f32, ramp: f32) -> f32 {
    match inside(events, t) {
        None => 0.0,
        Some((u, len)) => (u / ramp).min((len - u) / ramp).clamp(0.0, 1.0),
    }
}

/// Linear interpolation along an expression curve of (seconds, value).
fn curve(points: &[(f32, f32)], t: f32) -> f32 {
    match points.iter().rposition(|(at, _)| *at <= t) {
        None => points.first().map_or(1.0, |(_, v)| *v),
        Some(i) => match points.get(i + 1) {
            Some((next_at, next_v)) if *next_at > points[i].0 => {
                let f = ((t - points[i].0) / (next_at - points[i].0)).clamp(0.0, 1.0);
                points[i].1 + (next_v - points[i].1) * f
            }
            _ => points[i].1,
        },
    }
}

/// One individual of `kind` for `duration` seconds, boxed as a stereo voice.
/// `expression` is the channel's expression curve over the note.
pub fn texture_voice(
    kind: Texture,
    seed: u64,
    pitch: u8,
    duration: f32,
    gain: f32,
    pan_pos: f32,
    expression: Vec<(f32, f32)>,
) -> Box<dyn AudioUnit> {
    let expr = move |t: f32| curve(&expression, t);
    match kind {
        Texture::Crickets => {
            // A high sine under a pulse train, gated by seeded bursts. Pitch
            // nudges the species' carrier and spreads individuals across the field.
            let carrier = 3300.0 + (pitch as f32 - 60.0) * 45.0;
            let pulse_hz = 34.0 + hash01(seed, 7) * 10.0;
            let bursts = schedule(seed, duration, (0.4, 1.6), (0.3, 0.8));
            let env = lfo(move |t: f32| {
                let pulse = (0.5 + 0.5 * (std::f32::consts::TAU * pulse_hz * t).sin()).powf(4.0);
                gate(&bursts, t, 0.02) * pulse * expr(t)
            });
            let place = (pan_pos + (pitch as f32 - 62.0) / 8.0).clamp(-1.0, 1.0);
            Box::new((sine_hz(carrier) * env * (0.6 * gain)) >> pan(place))
        }
        Texture::Frogs => {
            // A short down-sweeping croak, rasped, repeating on a seeded schedule.
            // Lower pitch is a bigger frog.
            let f0 = 440.0 * 2f32.powf((pitch as f32 - 69.0) / 12.0);
            let calls = schedule(seed, duration, (1.5, 4.0), (0.22, 0.32));
            let calls_f = calls.clone();
            let freq = lfo(move |t: f32| match inside(&calls_f, t) {
                Some((u, len)) => f0 * (1.15 - 0.3 * (u / len)),
                None => f0,
            });
            let env = lfo(move |t: f32| match inside(&calls, t) {
                Some((u, len)) => {
                    let window = (std::f32::consts::PI * u / len).sin();
                    let rasp = (0.5 + 0.5 * (std::f32::consts::TAU * 22.0 * t).sin()).powf(2.0);
                    window * rasp * expr(t)
                }
                None => 0.0,
            });
            let place = (pan_pos + (pitch as f32 - 60.0) / 10.0).clamp(-1.0, 1.0);
            Box::new(
                (((freq >> triangle()) >> lowpass_hz(800.0, 0.7)) * env * (0.8 * gain))
                    >> pan(place),
            )
        }
        Texture::Breeze => {
            // Pink noise through a lowpass whose cutoff and level drift on slow,
            // incommensurate sines. Pitch is ignored; the seed sets the phases.
            let (p1, p2, p3, p4) = (
                hash01(seed, 1) * std::f32::consts::TAU,
                hash01(seed, 2) * std::f32::consts::TAU,
                hash01(seed, 3) * std::f32::consts::TAU,
                hash01(seed, 4) * std::f32::consts::TAU,
            );
            let cutoff = lfo(move |t: f32| {
                300.0
                    + 300.0 * (1.0 + (std::f32::consts::TAU * 0.07 * t + p1).sin())
                    + 150.0 * (1.0 + (std::f32::consts::TAU * 0.19 * t + p2).sin())
            });
            let level = lfo(move |t: f32| {
                let a = 0.5 * (1.0 + (std::f32::consts::TAU * 0.05 * t + p3).sin());
                let b = 0.5 * (1.0 + (std::f32::consts::TAU * 0.13 * t + p4).sin());
                (0.35 + 0.65 * a * b) * expr(t)
            });
            Box::new(
                (((pink() | cutoff | dc(0.7)) >> lowpass()) * level * (0.6 * gain)) >> pan(pan_pos),
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn render(kind: Texture, pitch: u8, seconds: f32) -> Wave {
        let mut unit = texture_voice(kind, 42, pitch, seconds, 1.0, 0.0, vec![(0.0, 1.0)]);
        Wave::render(22_050.0, seconds as f64, &mut *unit)
    }

    fn rms(w: &Wave) -> f32 {
        let ch = w.channel(0);
        (ch.iter().map(|x| x * x).sum::<f32>() / ch.len() as f32).sqrt()
    }

    /// Zero crossings per second on the left channel — a crude pitch-height read.
    fn crossings_per_second(w: &Wave) -> f32 {
        let ch = w.channel(0);
        let n = ch
            .windows(2)
            .filter(|p| (p[0] >= 0.0) != (p[1] >= 0.0))
            .count();
        n as f32 / w.duration() as f32
    }

    #[test]
    fn programs_round_trip_and_schedules_are_seeded_and_bounded() {
        for k in [Texture::Crickets, Texture::Frogs, Texture::Breeze] {
            assert_eq!(Texture::from_program(k.program()), Some(k));
            assert!(k.program() >= 128);
        }
        assert_eq!(Texture::from_program(73), None);
        let a = schedule(9, 20.0, (0.5, 1.5), (0.2, 0.4));
        let b = schedule(9, 20.0, (0.5, 1.5), (0.2, 0.4));
        assert_eq!(a, b);
        assert!(a.len() > 5);
        assert!(a
            .iter()
            .all(|(s, l)| *s >= 0.0 && *s < 20.0 && (0.2..=0.4).contains(l)));
        assert!(a.windows(2).all(|p| p[1].0 >= p[0].0 + p[0].1 + 0.5));
        assert_ne!(schedule(10, 20.0, (0.5, 1.5), (0.2, 0.4)), a);
    }

    #[test]
    fn crickets_sit_high_and_frogs_sit_low_and_the_breeze_blows() {
        let crickets = render(Texture::Crickets, 62, 4.0);
        let frogs = render(Texture::Frogs, 64, 8.0);
        let breeze = render(Texture::Breeze, 60, 4.0);
        assert!(rms(&crickets) > 0.005, "crickets silent");
        assert!(rms(&frogs) > 0.003, "frogs silent");
        assert!(rms(&breeze) > 0.01, "breeze silent");
        assert!(crossings_per_second(&crickets) > 1_800.0);
        assert!(crossings_per_second(&frogs) < 1_500.0);
        // The expression curve silences a texture like any other voice.
        let mut muted = texture_voice(
            Texture::Breeze,
            42,
            60,
            2.0,
            1.0,
            0.0,
            vec![(0.0, 0.0), (2.0, 0.0)],
        );
        let w = Wave::render(22_050.0, 2.0, &mut *muted);
        assert!(rms(&w) < 1e-4);
    }
}
