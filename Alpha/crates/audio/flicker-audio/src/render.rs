//! Offline rendering: a score to a stereo buffer, and the buffer to a 16-bit
//! WAV. This is the headless path the tests and CI use; no device involved.

use std::path::Path;

use fundsp::prelude32::Wave;

use crate::score::Score;
use crate::synth;

/// A finished offline render.
pub struct Rendered {
    pub wave: Wave,
    pub warnings: Vec<String>,
    pub voices: usize,
}

/// Render the whole score (plus the release tail) at `sample_rate`.
pub fn render(score: &Score, sample_rate: f64) -> Rendered {
    let built = synth::build(score, sample_rate);
    let mut master = built.master;
    let wave = Wave::render(sample_rate, built.duration_secs, &mut master);
    Rendered {
        wave,
        warnings: built.warnings,
        voices: built.voices,
    }
}

/// Largest absolute sample across all channels.
pub fn peak(wave: &Wave) -> f32 {
    (0..wave.channels())
        .flat_map(|c| wave.channel(c).iter().copied())
        .fold(0.0f32, |m, x| m.max(x.abs()))
}

/// Write 16-bit PCM, interleaved, clamped to full scale.
pub fn write_wav(wave: &Wave, path: &Path) -> Result<(), hound::Error> {
    let spec = hound::WavSpec {
        channels: wave.channels() as u16,
        sample_rate: wave.sample_rate() as u32,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(path, spec)?;
    for i in 0..wave.len() {
        for c in 0..wave.channels() {
            let x = wave.at(c, i).clamp(-1.0, 1.0);
            writer.write_sample((x * i16::MAX as f32).round() as i16)?;
        }
    }
    writer.finalize()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::smf;

    #[test]
    fn the_fixture_renders_audible_bounded_audio_of_the_right_length() {
        let score = smf::read(&smf::fixture::score_bytes()).unwrap();
        let sample_rate = 22_050.0;
        let r = render(&score, sample_rate);
        assert_eq!(r.wave.channels(), 2);
        let compiled = synth::build(&score, sample_rate).duration_secs;
        assert_eq!(r.wave.len(), (compiled * sample_rate).round() as usize);
        assert!(compiled >= score.duration_secs() + synth::TAIL_SECS);
        let p = peak(&r.wave);
        assert!(p > 0.02, "render is near-silent: peak {p}");
        assert!(p <= 1.0, "limiter let a sample through: peak {p}");
        assert!((0..r.wave.channels()).all(|c| r.wave.channel(c).iter().all(|x| x.is_finite())));
        // Hard-left pan on the last lead note: the left channel carries more of
        // the final bar than the right.
        let from = (score.seconds_at(crate::Pos::new(3, 4)) * sample_rate) as usize;
        let to = (score.duration_secs() * sample_rate) as usize;
        let energy = |c: usize| {
            r.wave.channel(c)[from..to]
                .iter()
                .map(|x| x * x)
                .sum::<f32>()
        };
        assert!(energy(0) > energy(1));
        assert_eq!(r.voices, 10);
        assert_eq!(r.warnings.len(), 1);
    }

    #[test]
    fn wav_round_trips_through_hound() {
        let score = smf::read(&smf::fixture::score_bytes()).unwrap();
        let r = render(&score, 8_000.0);
        let dir = std::env::temp_dir().join(format!("flicker-audio-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("fixture.wav");
        write_wav(&r.wave, &path).unwrap();
        let reader = hound::WavReader::open(&path).unwrap();
        assert_eq!(reader.spec().channels, 2);
        assert_eq!(reader.spec().sample_rate, 8_000);
        assert_eq!(reader.len() as usize, r.wave.len() * 2);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
