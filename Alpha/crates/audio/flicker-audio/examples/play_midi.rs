//! The slice-1 tester: read a Standard MIDI File into rows, print what the rows
//! say, then play it through the engine's own instruments — or render it to a
//! WAV when built without the `playback` feature (or when asked).
//!
//!   cargo run -p flicker-audio --features playback --example play_midi -- <file.mid>
//!   cargo run -p flicker-audio --example play_midi -- <file.mid> --wav out.wav [--rate 48000]
//!
//! The file is third-party content: pass its path, never commit it.

mod common;

use std::collections::BTreeMap;
use std::path::PathBuf;

use flicker_audio::patch::{program_name, KIT_CHANNEL};
use flicker_audio::score::Event;

struct Args {
    file: PathBuf,
    wav: Option<PathBuf>,
    rate: f64,
}

fn parse_args() -> Option<Args> {
    let mut args = std::env::args().skip(1);
    let file = PathBuf::from(args.next()?);
    let mut wav = None;
    let mut rate = 48_000.0;
    while let Some(flag) = args.next() {
        match flag.as_str() {
            "--wav" => wav = Some(PathBuf::from(args.next()?)),
            "--rate" => rate = args.next()?.parse().ok()?,
            _ => return None,
        }
    }
    Some(Args { file, wav, rate })
}

fn main() -> anyhow::Result<()> {
    let Some(args) = parse_args() else {
        eprintln!("usage: play_midi <file.mid> [--wav <out.wav>] [--rate <hz>]");
        std::process::exit(2);
    };
    let bytes = std::fs::read(&args.file)?;
    let (header, score) = flicker_audio::smf::read_with_header(&bytes)?;

    println!(
        "{}: format {}, {} tracks, {} ppq",
        args.file.display(),
        header.format,
        header.tracks,
        header.ppq
    );
    for t in &score.tempo {
        println!("  tempo {:>6.1} bpm at {}", t.bpm(), t.at);
    }
    if score.tempo.is_empty() {
        println!("  tempo  120.0 bpm (file default)");
    }
    for m in &score.meter {
        println!("  meter {}/{} at {}", m.numerator, m.denominator, m.at);
    }

    // Per channel: the patch bound first, note count, pitch range.
    let mut channels: BTreeMap<u8, (Option<u8>, usize, u8, u8)> = BTreeMap::new();
    for e in &score.events {
        match e {
            Event::Program(p) => {
                channels
                    .entry(p.channel)
                    .or_insert((None, 0, 127, 0))
                    .0
                    .get_or_insert(p.patch);
            }
            Event::Note(n) => {
                let c = channels.entry(n.channel).or_insert((None, 0, 127, 0));
                c.1 += 1;
                c.2 = c.2.min(n.pitch);
                c.3 = c.3.max(n.pitch);
            }
            Event::Control(_) => {}
        }
    }
    for (ch, (patch, notes, lo, hi)) in &channels {
        let voice = if *ch == KIT_CHANNEL {
            "kit".to_string()
        } else {
            let p = patch.unwrap_or(0);
            format!("{p:>3} {}", program_name(p))
        };
        println!(
            "  ch {:>2}  {voice:<28} {notes:>5} notes  pitch {lo}..{hi}",
            ch + 1
        );
    }
    println!(
        "  {} notes, {} whole notes, {:.2} s",
        score.notes().count(),
        score.end,
        score.duration_secs()
    );

    common::audition(&score, args.wav.as_deref(), args.rate)
}
