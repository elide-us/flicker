//! The instrument catalog: General MIDI's program numbers mapped to synthesis
//! recipes by FAMILY, the kit map, and the one tuning table.
//!
//! The recipes are a starter hypothesis — enough to hear a file through the
//! engine's own voices. They are rows; refining a sound is editing a row, not
//! code elsewhere. Anything unmapped resolves to a default that is audible and
//! REPORTED, never to silence.

use crate::texture::Texture;

/// How a program is voiced: a pitched recipe, or a soundscape texture whose
/// notes are individuals rather than tones.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Voice {
    Melodic(Recipe),
    Texture(Texture),
}

/// The voice for a patch index: General MIDI programs are recipes, the
/// engine-native programs past 127 are textures.
pub fn voice_for(program: u8) -> Voice {
    match Texture::from_program(program) {
        Some(t) => Voice::Texture(t),
        None => Voice::Melodic(recipe(program)),
    }
}

/// Frequency of absolute pitch index `pitch` under 12-tone equal temperament,
/// A4 (index 69) = 440 Hz. The single tuning table; a microtonal tuning swaps
/// this for a lookup.
pub fn hz(pitch: u8) -> f32 {
    440.0 * 2f32.powf((pitch as f32 - 69.0) / 12.0)
}

/// Frequency ratio of a pitch bend in semitones.
pub fn bend_ratio(semitones: f32) -> f32 {
    2f32.powf(semitones / 12.0)
}

/// The oscillator at the root of a melodic voice.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Osc {
    Sine,
    Triangle,
    Saw,
    SoftSaw,
    Square,
    /// Bandlimited pulse with this duty cycle (0..1; 0.5 is a square).
    Pulse(f32),
    Organ,
    Hammond,
    /// A sine whose pitch starts above the note and drops into it: struck
    /// skins (timpani, melodic toms, taiko).
    Struck,
}

/// How a melodic voice is built: oscillator → optional lowpass → ADSR → level.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Recipe {
    pub osc: Osc,
    /// Seconds to full level.
    pub attack: f32,
    /// Seconds from full level down to `sustain`.
    pub decay: f32,
    /// Held level, 0..1.
    pub sustain: f32,
    /// Seconds from the held level to silence after the note ends.
    pub release: f32,
    /// Lowpass cutoff in Hz; 0 means no filter.
    pub cutoff: f32,
    /// Level trim so families balance against each other.
    pub level: f32,
}

/// The sixteen General MIDI families, eight programs each.
pub const FAMILY_NAMES: [&str; 16] = [
    "Piano",
    "Chromatic Percussion",
    "Organ",
    "Guitar",
    "Bass",
    "Strings",
    "Ensemble",
    "Brass",
    "Reed",
    "Pipe",
    "Synth Lead",
    "Synth Pad",
    "Synth Effects",
    "Ethnic",
    "Percussive",
    "Sound Effects",
];

/// Family index (0..16) of a program.
pub fn family(program: u8) -> usize {
    (program as usize / 8).min(15)
}

/// The General MIDI level 1 program names, by program number.
pub const PROGRAM_NAMES: [&str; 128] = [
    "Acoustic Grand Piano",
    "Bright Acoustic Piano",
    "Electric Grand Piano",
    "Honky-tonk Piano",
    "Electric Piano 1",
    "Electric Piano 2",
    "Harpsichord",
    "Clavi",
    "Celesta",
    "Glockenspiel",
    "Music Box",
    "Vibraphone",
    "Marimba",
    "Xylophone",
    "Tubular Bells",
    "Dulcimer",
    "Drawbar Organ",
    "Percussive Organ",
    "Rock Organ",
    "Church Organ",
    "Reed Organ",
    "Accordion",
    "Harmonica",
    "Tango Accordion",
    "Acoustic Guitar (nylon)",
    "Acoustic Guitar (steel)",
    "Electric Guitar (jazz)",
    "Electric Guitar (clean)",
    "Electric Guitar (muted)",
    "Overdriven Guitar",
    "Distortion Guitar",
    "Guitar Harmonics",
    "Acoustic Bass",
    "Electric Bass (finger)",
    "Electric Bass (pick)",
    "Fretless Bass",
    "Slap Bass 1",
    "Slap Bass 2",
    "Synth Bass 1",
    "Synth Bass 2",
    "Violin",
    "Viola",
    "Cello",
    "Contrabass",
    "Tremolo Strings",
    "Pizzicato Strings",
    "Orchestral Harp",
    "Timpani",
    "String Ensemble 1",
    "String Ensemble 2",
    "Synth Strings 1",
    "Synth Strings 2",
    "Choir Aahs",
    "Voice Oohs",
    "Synth Voice",
    "Orchestra Hit",
    "Trumpet",
    "Trombone",
    "Tuba",
    "Muted Trumpet",
    "French Horn",
    "Brass Section",
    "Synth Brass 1",
    "Synth Brass 2",
    "Soprano Sax",
    "Alto Sax",
    "Tenor Sax",
    "Baritone Sax",
    "Oboe",
    "English Horn",
    "Bassoon",
    "Clarinet",
    "Piccolo",
    "Flute",
    "Recorder",
    "Pan Flute",
    "Blown Bottle",
    "Shakuhachi",
    "Whistle",
    "Ocarina",
    "Lead 1 (square)",
    "Lead 2 (sawtooth)",
    "Lead 3 (calliope)",
    "Lead 4 (chiff)",
    "Lead 5 (charang)",
    "Lead 6 (voice)",
    "Lead 7 (fifths)",
    "Lead 8 (bass + lead)",
    "Pad 1 (new age)",
    "Pad 2 (warm)",
    "Pad 3 (polysynth)",
    "Pad 4 (choir)",
    "Pad 5 (bowed)",
    "Pad 6 (metallic)",
    "Pad 7 (halo)",
    "Pad 8 (sweep)",
    "FX 1 (rain)",
    "FX 2 (soundtrack)",
    "FX 3 (crystal)",
    "FX 4 (atmosphere)",
    "FX 5 (brightness)",
    "FX 6 (goblins)",
    "FX 7 (echoes)",
    "FX 8 (sci-fi)",
    "Sitar",
    "Banjo",
    "Shamisen",
    "Koto",
    "Kalimba",
    "Bag pipe",
    "Fiddle",
    "Shanai",
    "Tinkle Bell",
    "Agogo",
    "Steel Drums",
    "Woodblock",
    "Taiko Drum",
    "Melodic Tom",
    "Synth Drum",
    "Reverse Cymbal",
    "Guitar Fret Noise",
    "Breath Noise",
    "Seashore",
    "Bird Tweet",
    "Telephone Ring",
    "Helicopter",
    "Applause",
    "Gunshot",
];

/// Name of a program, for readouts.
pub fn program_name(program: u8) -> &'static str {
    match Texture::from_program(program) {
        Some(t) => t.name(),
        None => PROGRAM_NAMES[(program & 0x7F) as usize],
    }
}

/// One recipe per family — the starter voices. Levels trim the families
/// against each other; the master limiter catches the sum.
const FAMILY_RECIPES: [Recipe; 16] = [
    // Piano: struck, decaying towards a soft held level.
    Recipe {
        osc: Osc::Triangle,
        attack: 0.003,
        decay: 0.5,
        sustain: 0.25,
        release: 0.3,
        cutoff: 3500.0,
        level: 1.0,
    },
    // Chromatic percussion: bells and music boxes ring and die.
    Recipe {
        osc: Osc::Sine,
        attack: 0.002,
        decay: 0.7,
        sustain: 0.0,
        release: 0.5,
        cutoff: 0.0,
        level: 1.0,
    },
    // Organ: instant on, flat, instant off.
    Recipe {
        osc: Osc::Organ,
        attack: 0.01,
        decay: 0.05,
        sustain: 1.0,
        release: 0.06,
        cutoff: 0.0,
        level: 0.7,
    },
    // Guitar: plucked, bright at the pick, damped.
    Recipe {
        osc: Osc::Pulse(0.3),
        attack: 0.003,
        decay: 0.6,
        sustain: 0.15,
        release: 0.25,
        cutoff: 2800.0,
        level: 0.9,
    },
    // Bass: round, holds.
    Recipe {
        osc: Osc::Triangle,
        attack: 0.005,
        decay: 0.35,
        sustain: 0.6,
        release: 0.12,
        cutoff: 1400.0,
        level: 1.1,
    },
    // Strings: bowed, slow in, slow out.
    Recipe {
        osc: Osc::Saw,
        attack: 0.12,
        decay: 0.2,
        sustain: 0.85,
        release: 0.35,
        cutoff: 4500.0,
        level: 0.6,
    },
    // Ensemble: wider, slower strings and voices.
    Recipe {
        osc: Osc::SoftSaw,
        attack: 0.2,
        decay: 0.3,
        sustain: 0.85,
        release: 0.5,
        cutoff: 3000.0,
        level: 0.6,
    },
    // Brass: bright, fast, assertive.
    Recipe {
        osc: Osc::Saw,
        attack: 0.03,
        decay: 0.1,
        sustain: 0.8,
        release: 0.15,
        cutoff: 6000.0,
        level: 0.65,
    },
    // Reed: hollow, held.
    Recipe {
        osc: Osc::Square,
        attack: 0.025,
        decay: 0.1,
        sustain: 0.8,
        release: 0.12,
        cutoff: 3200.0,
        level: 0.6,
    },
    // Pipe: pure, breathy attack.
    Recipe {
        osc: Osc::Sine,
        attack: 0.05,
        decay: 0.1,
        sustain: 0.9,
        release: 0.15,
        cutoff: 0.0,
        level: 0.9,
    },
    // Synth lead: the chip square.
    Recipe {
        osc: Osc::Square,
        attack: 0.004,
        decay: 0.1,
        sustain: 0.85,
        release: 0.1,
        cutoff: 0.0,
        level: 0.6,
    },
    // Synth pad: slow, soft, wide.
    Recipe {
        osc: Osc::SoftSaw,
        attack: 0.4,
        decay: 0.5,
        sustain: 0.9,
        release: 0.8,
        cutoff: 2200.0,
        level: 0.55,
    },
    // Synth effects: slow sines.
    Recipe {
        osc: Osc::Sine,
        attack: 0.3,
        decay: 0.4,
        sustain: 0.7,
        release: 0.6,
        cutoff: 0.0,
        level: 0.8,
    },
    // Ethnic: plucked, thin.
    Recipe {
        osc: Osc::Pulse(0.2),
        attack: 0.003,
        decay: 0.5,
        sustain: 0.1,
        release: 0.25,
        cutoff: 3500.0,
        level: 0.9,
    },
    // Percussive: struck, dies.
    Recipe {
        osc: Osc::Triangle,
        attack: 0.002,
        decay: 0.3,
        sustain: 0.0,
        release: 0.2,
        cutoff: 0.0,
        level: 1.0,
    },
    // Sound effects: a placeholder voice, not an attempt at the effect.
    Recipe {
        osc: Osc::Saw,
        attack: 0.02,
        decay: 0.3,
        sustain: 0.3,
        release: 0.3,
        cutoff: 1800.0,
        level: 0.6,
    },
];

/// The recipe for a program: its family's voice, with a few programs refined.
pub fn recipe(program: u8) -> Recipe {
    let program = program & 0x7F;
    let mut r = FAMILY_RECIPES[family(program)];
    match program {
        // Lead 2 is the sawtooth lead.
        81 => r.osc = Osc::Saw,
        // Synth basses are square and darker than the plucked basses.
        38 | 39 => {
            r.osc = Osc::Square;
            r.cutoff = 1200.0;
        }
        // Pizzicato strings and the harp are plucked, not bowed.
        45 => r = FAMILY_RECIPES[3],
        46 => {
            r = FAMILY_RECIPES[3];
            r.osc = Osc::Triangle;
        }
        // Timpani is a struck skin, not a bowed string.
        47 => {
            r = Recipe {
                osc: Osc::Struck,
                attack: 0.002,
                decay: 0.7,
                sustain: 0.0,
                release: 0.4,
                cutoff: 0.0,
                level: 0.9,
            }
        }
        // Taiko and melodic tom: struck, shorter.
        116 | 117 => {
            r.osc = Osc::Struck;
            r.decay = 0.5;
            r.level = 1.1;
        }
        _ => {}
    }
    r
}

/// The kit channel (MIDI channel 10, index 9): pitch means piece.
pub const KIT_CHANNEL: u8 = 9;

/// A one-shot percussion voice.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Piece {
    Kick,
    Snare,
    Stick,
    Clap,
    ClosedHat,
    OpenHat,
    /// A tom with this body frequency in Hz.
    Tom(f32),
    Crash,
    Ride,
    /// Engine-native piece 88: a buzz roll that lasts the note and swells —
    /// the one kit voice that reads its duration.
    SnareRoll,
}

/// The General MIDI percussion map — the pieces the engine voices today.
/// `None` is an unmapped piece: the synth plays its default and reports it.
pub fn piece(note: u8) -> Option<Piece> {
    Some(match note {
        35 | 36 => Piece::Kick,
        37 => Piece::Stick,
        38 | 40 => Piece::Snare,
        39 => Piece::Clap,
        41 => Piece::Tom(90.0),
        43 => Piece::Tom(110.0),
        45 => Piece::Tom(130.0),
        47 => Piece::Tom(150.0),
        48 => Piece::Tom(175.0),
        50 => Piece::Tom(200.0),
        42 | 44 => Piece::ClosedHat,
        46 => Piece::OpenHat,
        49 | 52 | 55 | 57 => Piece::Crash,
        51 | 53 | 59 => Piece::Ride,
        88 => Piece::SnareRoll,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_tuning_table_is_twelve_tone_equal_temperament() {
        assert!((hz(69) - 440.0).abs() < 1e-4);
        assert!((hz(81) - 880.0).abs() < 1e-3);
        assert!((hz(60) - 261.6256).abs() < 1e-3);
        assert!((bend_ratio(12.0) - 2.0).abs() < 1e-6);
        assert!((bend_ratio(0.0) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn every_program_resolves_to_a_recipe_and_a_name() {
        for program in 0..128u8 {
            let r = recipe(program);
            assert!(r.level > 0.0 && r.release > 0.0, "program {program}");
            assert!(!program_name(program).is_empty());
            assert_eq!(family(program), (program / 8) as usize);
        }
        assert_eq!(program_name(80), "Lead 1 (square)");
        assert_eq!(recipe(80).osc, Osc::Square);
        assert_eq!(recipe(81).osc, Osc::Saw);
        assert_eq!(FAMILY_NAMES[family(56)], "Brass");
        // Struck and plucked overrides inside bowed/percussive families.
        assert_eq!(recipe(47).osc, Osc::Struck);
        assert_eq!(recipe(47).sustain, 0.0);
        assert_eq!(recipe(116).osc, Osc::Struck);
        assert_eq!(recipe(117).osc, Osc::Struck);
        assert!(recipe(45).sustain < 0.3 && recipe(45).attack < 0.01);
        assert!(recipe(46).sustain < 0.3 && recipe(46).osc == Osc::Triangle);
        // The family default is untouched for a neighbour.
        assert_eq!(recipe(44).osc, Osc::Saw);
    }

    #[test]
    fn the_kit_maps_the_core_pieces_and_names_the_rest_unmapped() {
        assert_eq!(piece(36), Some(Piece::Kick));
        assert_eq!(piece(38), Some(Piece::Snare));
        assert_eq!(piece(42), Some(Piece::ClosedHat));
        assert_eq!(piece(75), None);
        let mapped = (0..128u8).filter(|n| piece(*n).is_some()).count();
        assert_eq!(mapped, 23);
        assert_eq!(piece(88), Some(Piece::SnareRoll));
        assert_eq!(program_name(128), "Crickets");
        assert!(matches!(voice_for(129), Voice::Texture(Texture::Frogs)));
        assert!(matches!(voice_for(47), Voice::Melodic(_)));
    }
}
