//! The mood tester: a FORM, four dials and a seed in, a generated piece out,
//! printed as a chart and then played (feature `playback`) or rendered.
//!
//!   cargo run -p flicker-audio --features playback --example play_mood -- --form ambient --seed 7
//!   cargo run -p flicker-audio --example play_mood -- --form loop --arc swell --bars 16 --wav swell.wav
//!
//! Flags: --form ambient|night|loop · --tension --brightness --density --pace (0..1) · --seed <n>
//!        --bars <n> (loop form only) · --arc flat|rise|fall|swell · --tonic C|C#|D|…|B
//!        --wav <out.wav> · --rate <hz>

mod common;

use std::path::PathBuf;

use flicker_audio::arrangement::{compose, Form, Harmony};
use flicker_audio::mood::{bpm_for, role_chart, Arc, Dials};
use flicker_audio::theory::PITCH_NAMES;

struct Args {
    form: String,
    dials: Dials,
    seed: u64,
    bars: u32,
    arc: Arc,
    tonic: u8,
    wav: Option<PathBuf>,
    rate: f64,
}

fn parse_args() -> Option<Args> {
    let mut a = Args {
        form: "ambient".to_string(),
        dials: Dials {
            tension: 0.4,
            brightness: 0.6,
            density: 0.4,
            pace: 0.25,
        },
        seed: 7,
        bars: 16,
        arc: Arc::Flat,
        tonic: 0,
        wav: None,
        rate: 48_000.0,
    };
    let mut args = std::env::args().skip(1);
    while let Some(flag) = args.next() {
        let value = args.next()?;
        match flag.as_str() {
            "--form" => a.form = value,
            "--tension" => a.dials.tension = value.parse().ok()?,
            "--brightness" => a.dials.brightness = value.parse().ok()?,
            "--density" => a.dials.density = value.parse().ok()?,
            "--pace" => a.dials.pace = value.parse().ok()?,
            "--seed" => a.seed = value.parse().ok()?,
            "--bars" => a.bars = value.parse().ok()?,
            "--arc" => a.arc = Arc::parse(&value)?,
            "--tonic" => {
                a.tonic = PITCH_NAMES
                    .iter()
                    .position(|n| n.eq_ignore_ascii_case(&value))? as u8
            }
            "--wav" => a.wav = Some(PathBuf::from(value)),
            "--rate" => a.rate = value.parse().ok()?,
            _ => return None,
        }
    }
    Some(a)
}

fn main() -> anyhow::Result<()> {
    let Some(args) = parse_args() else {
        eprintln!(
            "usage: play_mood [--form ambient|night|loop] [--tension x] [--brightness x] [--density x] [--pace x] [--seed n] [--bars n] [--arc flat|rise|fall|swell] [--tonic C..B] [--wav out.wav] [--rate hz]"
        );
        std::process::exit(2);
    };
    let Some(form) = Form::parse(&args.form, args.bars) else {
        eprintln!("unknown form {:?}: ambient, night or loop", args.form);
        std::process::exit(2);
    };

    let bars = form.bars();
    let trajectory = args.arc.trajectory(args.dials, bars);
    let piece = compose(&form, args.seed, &trajectory, args.tonic);
    let d = args.dials;
    println!(
        "form {} · seed {} · {} bars · 4/4 · tonic {} · arc {:?} · dials tension {:.2} brightness {:.2} density {:.2} pace {:.2}",
        form.name,
        piece.seed,
        bars,
        PITCH_NAMES[piece.tonic as usize],
        args.arc,
        d.tension,
        d.brightness,
        d.density,
        d.pace
    );
    println!(
        "tempo {:.0} bpm at the dials · roles: {}",
        bpm_for(d.pace),
        role_chart(d.brightness)
    );
    for s in &piece.sections {
        let layers: Vec<String> = s
            .layers
            .iter()
            .map(|r| format!("{r:?}").to_lowercase())
            .collect();
        let harmony = match s.harmony {
            Harmony::Tonic => "tonic drone".to_string(),
            Harmony::Bars(n) => format!("{n} bars per chord"),
            Harmony::Pace => "chords by pace".to_string(),
        };
        println!(
            "== {:<12} bars {:>2}–{:<3} {:<34} {}",
            s.name,
            s.start_bar + 1,
            s.start_bar + s.bars,
            layers.join(" "),
            harmony
        );
        for p in &piece.phrases[s.phrases.clone()] {
            print_phrase(p);
        }
    }
    if piece.phrases.iter().any(|p| p.lifts > 0) {
        println!("  * = lifted to the dominant to reach the tension target");
    }
    println!(
        "  {} notes, {:.2} s",
        piece.score.notes().count(),
        piece.score.duration_secs()
    );

    common::audition(&piece.score, args.wav.as_deref(), args.rate)
}

fn print_phrase(p: &flicker_audio::mood::Phrase) {
    println!(
        "bars {:>2}–{:<2} {:<10} {:<12} {:<9} {}    tension {:.2} → {:.2}",
        p.start_bar + 1,
        p.start_bar + p.bars,
        p.mode.name(),
        p.template,
        p.pattern,
        p.numerals(),
        p.target_tension,
        p.achieved_tension
    );
    if let Some(m) = &p.melody {
        let variations: Vec<String> = m
            .variations
            .iter()
            .skip(1)
            .map(|v| format!("{v:?}").to_lowercase())
            .collect();
        println!(
            "           melody {}·{}{} · variations {} · departure {:?} via {}",
            m.motif.cell,
            m.motif.contour,
            if m.phrase_departure {
                " (contrast)"
            } else {
                ""
            },
            if variations.is_empty() {
                "-".to_string()
            } else {
                variations.join(", ")
            },
            m.departure,
            m.departure_cell
        );
    }
}
