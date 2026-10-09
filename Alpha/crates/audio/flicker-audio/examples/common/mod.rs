//! The tail every tester shares: render the score to a WAV when asked,
//! otherwise play it live (feature `playback`) and report underruns.

use std::path::Path;

use flicker_audio::score::Score;

#[allow(dead_code)]
pub fn audition(score: &Score, wav: Option<&Path>, rate: f64) -> anyhow::Result<()> {
    if let Some(out) = wav {
        let r = flicker_audio::render::render(score, rate);
        for w in &r.warnings {
            println!("  warning: {w}");
        }
        flicker_audio::render::write_wav(&r.wave, out)?;
        println!(
            "rendered {} — {} voices, {:.2} s at {} Hz, peak {:.3}",
            out.display(),
            r.voices,
            r.wave.duration(),
            rate,
            flicker_audio::render::peak(&r.wave)
        );
        return Ok(());
    }

    #[cfg(feature = "playback")]
    {
        let playing = flicker_audio::play::start(score)?;
        for w in &playing.report.warnings {
            println!("  warning: {w}");
        }
        println!(
            "playing — {} voices, {:.2} s, device {} Hz × {} ch",
            playing.report.voices,
            playing.report.duration_secs,
            playing.report.sample_rate,
            playing.report.channels
        );
        let underruns = playing.wait();
        if underruns > 0 {
            println!("WARNING: {underruns} audio underruns — the callback missed deadlines; what you heard had gaps");
        } else {
            println!("done, no underruns");
        }
        Ok(())
    }
    #[cfg(not(feature = "playback"))]
    {
        eprintln!("built without the `playback` feature: add --features playback to hear it, or --wav <out.wav> to render it");
        std::process::exit(2);
    }
}
