//! Music theory as ROWS. One major-scale row; every mode is a rotation of it,
//! every diatonic chord is thirds stacked from the mode, and a chord's quality
//! is read off its intervals — nothing here is a second copy of something that
//! can be derived. The progression templates and the function/tension weights
//! are the starter hypotheses the generator renders from; they are rows so the
//! ear can change them without touching code.

/// The major scale, in semitones above the tonic. Every mode rotates this.
pub const MAJOR_STEPS: [u8; 7] = [0, 2, 4, 5, 7, 9, 11];

/// The seven diatonic modes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Lydian,
    Ionian,
    Mixolydian,
    Dorian,
    Aeolian,
    Phrygian,
    Locrian,
}

/// The lightness ladder, brightest first: raise or lower one note at a time.
pub const LIGHTNESS: [Mode; 7] = [
    Mode::Lydian,
    Mode::Ionian,
    Mode::Mixolydian,
    Mode::Dorian,
    Mode::Aeolian,
    Mode::Phrygian,
    Mode::Locrian,
];

impl Mode {
    /// Which degree of the major scale this mode starts on.
    fn rotation(self) -> usize {
        match self {
            Mode::Ionian => 0,
            Mode::Dorian => 1,
            Mode::Phrygian => 2,
            Mode::Lydian => 3,
            Mode::Mixolydian => 4,
            Mode::Aeolian => 5,
            Mode::Locrian => 6,
        }
    }

    /// Semitones above the tonic for each of the seven degrees.
    pub fn steps(self) -> [u8; 7] {
        let r = self.rotation();
        let mut s = [0u8; 7];
        for (i, step) in s.iter_mut().enumerate() {
            *step = (MAJOR_STEPS[(r + i) % 7] + 12 - MAJOR_STEPS[r]) % 12;
        }
        s
    }

    pub fn name(self) -> &'static str {
        match self {
            Mode::Lydian => "Lydian",
            Mode::Ionian => "Ionian",
            Mode::Mixolydian => "Mixolydian",
            Mode::Dorian => "Dorian",
            Mode::Aeolian => "Aeolian",
            Mode::Phrygian => "Phrygian",
            Mode::Locrian => "Locrian",
        }
    }

    /// The mode a brightness dial (0 dark .. 1 bright) lands on.
    pub fn for_brightness(brightness: f32) -> Mode {
        let i = ((1.0 - brightness.clamp(0.0, 1.0)) * 6.0).round() as usize;
        LIGHTNESS[i.min(6)]
    }

    /// Major family when the tonic triad's third is major.
    pub fn is_major_family(self) -> bool {
        self.steps()[2] == 4
    }

    /// Is this pitch class (semitones above the tonic) in the mode?
    pub fn contains(self, pc: u8) -> bool {
        self.steps().contains(&(pc % 12))
    }
}

/// Harmonic function of a degree.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Function {
    Tonic,
    Predominant,
    Dominant,
}

/// Degree → function, major-family modes.
pub const FUNCTION_MAJOR: [Function; 7] = [
    Function::Tonic,
    Function::Predominant,
    Function::Tonic,
    Function::Predominant,
    Function::Dominant,
    Function::Tonic,
    Function::Dominant,
];

/// Degree → function, minor-family modes (the submediant leans pre-dominant).
pub const FUNCTION_MINOR: [Function; 7] = [
    Function::Tonic,
    Function::Predominant,
    Function::Tonic,
    Function::Predominant,
    Function::Dominant,
    Function::Predominant,
    Function::Dominant,
];

/// Tension weight of each function: tonic, pre-dominant, dominant.
pub const FUNCTION_TENSION: [f32; 3] = [0.0, 0.45, 0.9];

pub fn function(mode: Mode, degree: usize) -> Function {
    let row = if mode.is_major_family() {
        &FUNCTION_MAJOR
    } else {
        &FUNCTION_MINOR
    };
    row[degree % 7]
}

/// Chord quality, read off the intervals.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Quality {
    Major,
    Minor,
    Diminished,
    Augmented,
    Major7,
    Dominant7,
    Minor7,
    HalfDiminished7,
    MinorMajor7,
    Other,
}

/// A chord on a degree of a mode: tones are semitones above the TONIC,
/// ascending, so the voicing step can place them freely.
#[derive(Clone, Debug, PartialEq)]
pub struct Chord {
    pub degree: u8,
    pub tones: Vec<u8>,
    pub quality: Quality,
    pub function: Function,
    pub tension: f32,
}

impl Chord {
    /// Pitch classes (0..12) relative to the tonic.
    pub fn pitch_classes(&self) -> Vec<u8> {
        self.tones.iter().map(|t| t % 12).collect()
    }
}

/// Stack `tones` thirds (3 = triad, 4 = seventh) from `degree` of `mode`.
pub fn diatonic_chord(mode: Mode, degree: usize, tones: usize) -> Chord {
    let s = mode.steps();
    let degree = degree % 7;
    let mut out: Vec<u8> = Vec::with_capacity(tones);
    let mut last: i32 = s[degree] as i32 - 1;
    for k in 0..tones.max(3) {
        let mut t = s[(degree + 2 * k) % 7] as i32;
        while t <= last {
            t += 12;
        }
        out.push(t as u8);
        last = t;
    }
    let quality = quality_of(&out);
    let function = function(mode, degree);
    let tension = chord_tension(function, out.len(), quality, degree);
    Chord {
        degree: degree as u8,
        tones: out,
        quality,
        function,
        tension,
    }
}

fn quality_of(tones: &[u8]) -> Quality {
    let third = tones[1] - tones[0];
    let fifth = tones[2] - tones[0];
    let seventh = tones.get(3).map(|t| t - tones[0]);
    match (third, fifth, seventh) {
        (4, 7, None) => Quality::Major,
        (3, 7, None) => Quality::Minor,
        (3, 6, None) => Quality::Diminished,
        (4, 8, None) => Quality::Augmented,
        (4, 7, Some(11)) => Quality::Major7,
        (4, 7, Some(10)) => Quality::Dominant7,
        (3, 7, Some(10)) => Quality::Minor7,
        (3, 6, Some(10)) => Quality::HalfDiminished7,
        (3, 7, Some(11)) => Quality::MinorMajor7,
        _ => Quality::Other,
    }
}

/// The tension row: function weight, plus a seventh, plus being away from
/// home, plus a diminished colour. Clamped to 0..1.
pub fn chord_tension(function: Function, tones: usize, quality: Quality, degree: usize) -> f32 {
    let mut t = FUNCTION_TENSION[function as usize];
    if tones >= 4 {
        t += 0.1;
    }
    if degree != 0 {
        t += 0.05;
    }
    if matches!(quality, Quality::Diminished | Quality::HalfDiminished7) {
        t += 0.1;
    }
    t.clamp(0.0, 1.0)
}

/// Roman numeral for readouts: case from the third, suffix from the rest.
pub fn roman(chord: &Chord) -> String {
    const NUMERALS: [&str; 7] = ["I", "II", "III", "IV", "V", "VI", "VII"];
    let numeral = NUMERALS[chord.degree as usize % 7];
    let minor = matches!(
        chord.quality,
        Quality::Minor
            | Quality::Diminished
            | Quality::Minor7
            | Quality::HalfDiminished7
            | Quality::MinorMajor7
    );
    let base = if minor {
        numeral.to_lowercase()
    } else {
        numeral.to_string()
    };
    let suffix = match chord.quality {
        Quality::Major | Quality::Minor => "",
        Quality::Diminished => "°",
        Quality::Augmented => "+",
        Quality::Major7 => "M7",
        Quality::Dominant7 | Quality::Minor7 => "7",
        Quality::HalfDiminished7 => "ø7",
        Quality::MinorMajor7 => "mM7",
        Quality::Other => "?",
    };
    format!("{base}{suffix}")
}

/// A progression template: degrees (0-based) in order. The generator stretches
/// a template over a phrase's chord slots and scores its tension curve against
/// the trajectory.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Template {
    pub name: &'static str,
    pub degrees: &'static [u8],
}

/// The starter templates — the classical and popular shapes the spec names.
pub const TEMPLATES: [Template; 12] = [
    Template {
        name: "home",
        degrees: &[0, 0, 3, 0],
    },
    Template {
        name: "plagal",
        degrees: &[0, 3, 0, 3],
    },
    Template {
        name: "vamp",
        degrees: &[0, 1, 0, 1],
    },
    Template {
        name: "axis",
        degrees: &[0, 4, 5, 3],
    },
    Template {
        name: "fifties",
        degrees: &[0, 5, 3, 4],
    },
    Template {
        name: "turnaround",
        degrees: &[0, 1, 4, 0],
    },
    Template {
        name: "two-five-one",
        degrees: &[1, 4, 0],
    },
    Template {
        name: "deceptive",
        degrees: &[0, 3, 4, 5],
    },
    Template {
        name: "pachelbel",
        degrees: &[0, 4, 5, 2, 3, 0, 3, 4],
    },
    Template {
        name: "minor-epic",
        degrees: &[0, 5, 2, 6],
    },
    Template {
        name: "andalusian",
        degrees: &[0, 6, 5, 4],
    },
    Template {
        name: "circle",
        degrees: &[0, 3, 6, 2, 5, 1, 4, 0],
    },
];

/// Pitch-class names for readouts (sharps).
pub const PITCH_NAMES: [&str; 12] = [
    "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modes_are_rotations_of_one_row() {
        assert_eq!(Mode::Ionian.steps(), [0, 2, 4, 5, 7, 9, 11]);
        assert_eq!(Mode::Aeolian.steps(), [0, 2, 3, 5, 7, 8, 10]);
        assert_eq!(Mode::Lydian.steps(), [0, 2, 4, 6, 7, 9, 11]);
        assert_eq!(Mode::Locrian.steps(), [0, 1, 3, 5, 6, 8, 10]);
        assert_eq!(Mode::Dorian.steps(), [0, 2, 3, 5, 7, 9, 10]);
        assert!(Mode::Ionian.is_major_family() && Mode::Lydian.is_major_family());
        assert!(!Mode::Aeolian.is_major_family() && !Mode::Locrian.is_major_family());
        // The ladder lightens by one accidental per step.
        for pair in LIGHTNESS.windows(2) {
            let (a, b) = (pair[0].steps(), pair[1].steps());
            let changed = a.iter().zip(b.iter()).filter(|(x, y)| x != y).count();
            assert_eq!(changed, 1, "{:?} → {:?}", pair[0], pair[1]);
        }
        assert_eq!(Mode::for_brightness(1.0), Mode::Lydian);
        assert_eq!(Mode::for_brightness(0.5), Mode::Dorian);
        assert_eq!(Mode::for_brightness(0.0), Mode::Locrian);
    }

    #[test]
    fn chords_stack_from_the_mode_and_read_their_quality() {
        let one = diatonic_chord(Mode::Ionian, 0, 3);
        assert_eq!(one.tones, vec![0, 4, 7]);
        assert_eq!(one.quality, Quality::Major);
        assert_eq!(roman(&one), "I");
        let five7 = diatonic_chord(Mode::Ionian, 4, 4);
        assert_eq!(five7.tones, vec![7, 11, 14, 17]);
        assert_eq!(five7.quality, Quality::Dominant7);
        assert_eq!(roman(&five7), "V7");
        let seven = diatonic_chord(Mode::Ionian, 6, 3);
        assert_eq!(seven.quality, Quality::Diminished);
        assert_eq!(roman(&seven), "vii°");
        assert_eq!(roman(&diatonic_chord(Mode::Ionian, 6, 4)), "viiø7");
        assert_eq!(roman(&diatonic_chord(Mode::Ionian, 1, 4)), "ii7");
        assert_eq!(roman(&diatonic_chord(Mode::Ionian, 0, 4)), "IM7");
        assert_eq!(roman(&diatonic_chord(Mode::Aeolian, 0, 3)), "i");
        assert_eq!(roman(&diatonic_chord(Mode::Aeolian, 5, 3)), "VI");
        assert_eq!(roman(&diatonic_chord(Mode::Lydian, 1, 3)), "II");
        // Every diatonic seventh in every mode is a named quality.
        for mode in LIGHTNESS {
            for d in 0..7 {
                assert_ne!(
                    diatonic_chord(mode, d, 4).quality,
                    Quality::Other,
                    "{mode:?} {d}"
                );
                assert_ne!(
                    diatonic_chord(mode, d, 3).quality,
                    Quality::Other,
                    "{mode:?} {d}"
                );
            }
        }
    }

    #[test]
    fn tension_rises_from_tonic_through_predominant_to_dominant() {
        let t = |d| diatonic_chord(Mode::Ionian, d, 3).tension;
        assert!(t(0) < t(3) && t(3) < t(4));
        assert!(diatonic_chord(Mode::Ionian, 4, 4).tension > t(4));
        assert!(diatonic_chord(Mode::Ionian, 6, 3).tension > t(4));
        assert_eq!(t(0), 0.0);
        assert!(TEMPLATES.iter().all(|t| t.degrees.iter().all(|d| *d < 7)));
    }
}
