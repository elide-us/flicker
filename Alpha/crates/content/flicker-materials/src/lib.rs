//! flicker-materials — the world-material **vocabulary** (data-model tier ①).
//!
//! This crate is the typed, in-memory form of `Alpha/content/data/*.json`: the
//! periodic table ([`Element`]) and the 256-entry material index
//! ([`MaterialDef`]). It is the *reference* layer the whole material model
//! classifies against — see `docs/material-model-handoff.md` §1–2.
//!
//! Three pieces:
//!
//! - [`TableSource`] — the loader **seam**. The simulation asks a source for
//!   the rows; it never hardcodes a path. [`JsonTableSource`] reads the JSON
//!   files today; a flicker-net → web service → DB source plugs in behind the
//!   same trait later, with no caller change.
//! - [`Tables`] — the loaded vocabulary, indexed for lookup by element symbol /
//!   atomic number and by material id / name.
//! - [`Tables::blend_traits`] — the composition-weighted trait blend
//!   (Σ fractionᵢ·traitᵢ): a raw element composition's *effective* element
//!   traits, the fallback basis a formed material then overrides.
//!
//! Deliberately **not** here (a flagged TBD in the handoff §6): the classifier
//! that maps a composition to one of the 256 material ids. This crate supplies
//! the vocabulary that classifier will read; it does not guess the rules.

pub mod compound;
pub mod element;
pub mod material;
pub mod rock;
pub mod source;
pub mod tables;

pub use compound::{CompoundDef, CompoundElement, MetamorphicRule};
pub use element::{Element, ElementId, PhysicalState};
pub use material::{MaterialDef, MaterialId};
pub use rock::RockDef;
pub use source::{JsonTableSource, MaterialError, TableSource};
pub use tables::{ElementTraits, Tables};
