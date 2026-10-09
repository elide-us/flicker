//! Musical time: exact fractions of a whole note, and the tempo rows that turn
//! them into seconds.
//!
//! Every position and duration in the symbolic layer is a [`Pos`]. There is no
//! tick grid to pick a resolution for: a triplet eighth is 1/12, a quintuplet
//! sixteenth 1/20, swing puts the off-beat at 2/3 of the beat, and nested
//! tuplets stay exact. Seconds exist only on the far side of
//! [`seconds_at`], which walks the tempo rows once per conversion.

use core::cmp::Ordering;
use core::fmt;
use core::ops::{Add, Sub};

/// A position or duration in WHOLE-NOTE units, always reduced, denominator > 0.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Pos {
    num: i64,
    den: i64,
}

impl Pos {
    /// The origin / the zero duration.
    pub const ZERO: Pos = Pos { num: 0, den: 1 };

    /// `num / den` whole notes. Panics on a zero denominator.
    pub fn new(num: i64, den: i64) -> Pos {
        assert!(den != 0, "a musical position needs a non-zero denominator");
        reduce(num as i128, den as i128)
    }

    /// A tick count at `ppq` pulses per quarter note: `ticks / (4 * ppq)` whole
    /// notes, exact — the file's triplets land as thirds, not rounded ticks.
    pub fn from_ticks(ticks: u64, ppq: u16) -> Pos {
        assert!(ppq > 0, "pulses per quarter must be positive");
        reduce(ticks as i128, 4 * ppq as i128)
    }

    pub fn num(self) -> i64 {
        self.num
    }

    pub fn den(self) -> i64 {
        self.den
    }

    pub fn is_zero(self) -> bool {
        self.num == 0
    }

    /// This position scaled by `num / den` — a slot length times a pattern fraction.
    pub fn scaled(self, num: i64, den: i64) -> Pos {
        assert!(den != 0, "a scale needs a non-zero denominator");
        reduce(
            self.num as i128 * num as i128,
            self.den as i128 * den as i128,
        )
    }

    /// Whole notes as a float — for display and for the seconds conversion only.
    pub fn to_f64(self) -> f64 {
        self.num as f64 / self.den as f64
    }
}

fn gcd(mut a: i128, mut b: i128) -> i128 {
    while b != 0 {
        let t = a % b;
        a = b;
        b = t;
    }
    a
}

fn reduce(num: i128, den: i128) -> Pos {
    let (num, den) = if den < 0 { (-num, -den) } else { (num, den) };
    let g = gcd(num.abs(), den).max(1);
    let (num, den) = (num / g, den / g);
    Pos {
        num: i64::try_from(num).expect("musical position overflow"),
        den: i64::try_from(den).expect("musical position overflow"),
    }
}

impl Add for Pos {
    type Output = Pos;
    fn add(self, o: Pos) -> Pos {
        reduce(
            self.num as i128 * o.den as i128 + o.num as i128 * self.den as i128,
            self.den as i128 * o.den as i128,
        )
    }
}

impl Sub for Pos {
    type Output = Pos;
    fn sub(self, o: Pos) -> Pos {
        reduce(
            self.num as i128 * o.den as i128 - o.num as i128 * self.den as i128,
            self.den as i128 * o.den as i128,
        )
    }
}

impl PartialOrd for Pos {
    fn partial_cmp(&self, o: &Pos) -> Option<Ordering> {
        Some(self.cmp(o))
    }
}

impl Ord for Pos {
    fn cmp(&self, o: &Pos) -> Ordering {
        (self.num as i128 * o.den as i128).cmp(&(o.num as i128 * self.den as i128))
    }
}

impl Default for Pos {
    fn default() -> Self {
        Pos::ZERO
    }
}

impl fmt::Display for Pos {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.den == 1 {
            write!(f, "{}", self.num)
        } else {
            write!(f, "{}/{}", self.num, self.den)
        }
    }
}

/// One tempo row: from `at` onward a whole note lasts `whole_secs` seconds.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TempoChange {
    pub at: Pos,
    pub whole_secs: f64,
}

impl TempoChange {
    /// From a Standard MIDI File tempo (microseconds per quarter note).
    pub fn from_micros_per_quarter(at: Pos, micros: u32) -> TempoChange {
        TempoChange {
            at,
            whole_secs: micros as f64 * 4.0 / 1_000_000.0,
        }
    }

    /// Quarter-note beats per minute, for readouts.
    pub fn bpm(&self) -> f64 {
        240.0 / self.whole_secs
    }
}

/// A whole note at the Standard MIDI File default of 120 bpm.
pub const DEFAULT_WHOLE_SECS: f64 = 2.0;

/// Seconds from the origin to `pos`, walking `tempo` (sorted by `at`). Before
/// the first row, and with no rows at all, the default tempo applies.
pub fn seconds_at(tempo: &[TempoChange], pos: Pos) -> f64 {
    let mut secs = 0.0;
    let mut cursor = Pos::ZERO;
    let mut whole = DEFAULT_WHOLE_SECS;
    for t in tempo {
        if t.at >= pos {
            break;
        }
        if t.at > cursor {
            secs += (t.at - cursor).to_f64() * whole;
            cursor = t.at;
        }
        whole = t.whole_secs;
    }
    secs + (pos - cursor).to_f64() * whole
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ticks_land_as_exact_fractions() {
        // 480 ppq: a quarter is 480 ticks, a triplet eighth 160, a dotted eighth 360.
        assert_eq!(Pos::from_ticks(480, 480), Pos::new(1, 4));
        assert_eq!(Pos::from_ticks(160, 480), Pos::new(1, 12));
        assert_eq!(Pos::from_ticks(360, 480), Pos::new(3, 16));
        assert_eq!(Pos::from_ticks(0, 96), Pos::ZERO);
        // 96 ppq quintuplet sixteenth: a quarter in five = 96/5 ticks is not
        // integral, which is exactly the tick-grid problem; in rationals it is 1/20.
        assert_eq!(Pos::new(1, 4) - Pos::new(1, 5), Pos::new(1, 20));
    }

    #[test]
    fn arithmetic_reduces_and_orders() {
        let a = Pos::new(1, 4) + Pos::new(1, 12); // 1/3
        assert_eq!(a, Pos::new(1, 3));
        assert_eq!((a.num(), a.den()), (1, 3));
        assert!(Pos::new(1, 3) < Pos::new(5, 12));
        assert!(Pos::new(2, 4) == Pos::new(1, 2));
        assert_eq!(Pos::new(-1, -2), Pos::new(1, 2));
        assert_eq!(Pos::new(1, 2).scaled(1, 3), Pos::new(1, 6));
        assert_eq!(Pos::new(1, 1).scaled(3, 4), Pos::new(3, 4));
        assert_eq!(format!("{}", Pos::new(3, 8)), "3/8");
        assert_eq!(format!("{}", Pos::new(8, 4)), "2");
    }

    #[test]
    fn seconds_follow_the_tempo_rows() {
        // No rows: 120 bpm, a whole note is 2 s.
        assert_eq!(seconds_at(&[], Pos::new(1, 4)), 0.5);
        // 120 bpm for one bar, then 60 bpm.
        let tempo = [
            TempoChange::from_micros_per_quarter(Pos::ZERO, 500_000),
            TempoChange::from_micros_per_quarter(Pos::new(1, 1), 1_000_000),
        ];
        assert!((tempo[1].bpm() - 60.0).abs() < 1e-9);
        assert_eq!(seconds_at(&tempo, Pos::new(1, 1)), 2.0);
        assert_eq!(seconds_at(&tempo, Pos::new(3, 2)), 2.0 + 2.0);
        // A row that starts late leaves the default in force before it.
        let late = [TempoChange::from_micros_per_quarter(
            Pos::new(1, 2),
            250_000,
        )];
        assert_eq!(seconds_at(&late, Pos::new(1, 1)), 1.0 + 0.5);
    }
}
