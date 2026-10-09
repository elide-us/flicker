//! flicker-audio — the audio **engine**: synthesis, sequencing, mixing, output.
//!
//! # Slice 1: play a Standard MIDI File through our own instruments
//!
//! ```text
//!   .mid bytes ──▶ smf::read ──▶ Score (rows) ──▶ synth::build ──▶ fundsp Net ──▶ render (WAV) / play (device)
//!                  the ONE place      exact time,      GM patch table,     reverb bus,
//!                  MIDI exists        notes + lanes    voices, strips      limiter
//! ```
//!
//! - **Musical time is exact.** A [`time::Pos`] is a fraction of a whole note
//!   (a triplet eighth is 1/12, a dotted eighth 3/16). Conversion to seconds
//!   happens once, through the tempo rows, at the sequencer boundary.
//! - **The event rows are the contract.** [`score::Score`] is what the
//!   generator will later write directly; the MIDI reader is a thin adapter
//!   that fills the same rows from a file.
//! - **Pitch is one number.** The absolute pitch index (69 = A4) resolves to
//!   hertz through [`patch::hz`], the single tuning table.
//! - **Instruments are rows.** [`patch::recipe`] maps a General MIDI program
//!   to a synthesis recipe by family; the kit maps piece numbers; anything
//!   unmapped falls back audibly and is reported, never silent.
//!
//! # Slice 2: a progression engine you can hear from dials
//!
//! [`theory`] holds music theory as rows (modes on a lightness ladder, chords
//! stacked from the mode, degree functions, progression templates), [`mood`]
//! turns four dials — tension, brightness, density, pace — plus a seed into the
//! same [`score::Score`] rows the MIDI reader produces, [`melody`] lays a
//! motif over that harmony under the rule of threes, and [`arrangement`]
//! stretches it all over time as a FORM: sections whose layers enter and leave
//! with fades on the expression lane. The `play_mood` example is their tester.
//!
//! The crate has no scene, no UI and no file format of its own yet; the
//! examples are the testers.

pub mod arrangement;
pub mod melody;
pub mod mood;
pub mod patch;
pub mod render;
pub mod score;
pub mod smf;
pub mod synth;
pub mod texture;
pub mod theory;
pub mod time;

#[cfg(feature = "playback")]
pub mod play;

pub use score::{Control, ControlKind, Event, Meter, Note, Program, Score};
pub use time::{Pos, TempoChange};
