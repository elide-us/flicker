//! flicker-worldstate — the retained world truth (data-model **tier ②**).
//!
//! This crate is the spec's `world-state` (system spec §13): the authority that
//! holds the material **ledger** and, later, evolves it. This slice builds only
//! the *schema* — the data structures; the simulation that changes them is
//! deferred.
//!
//! # What a pixel actually is (system spec §4, handoff §1)
//!
//! Each surface column is a [`Cell`]: a [`Composition`] (element → absolute
//! mass), the [`Composition`] of the solid column beneath it
//! ([`Cell::bulk_composition`]), a classified [`MaterialId`] face
//! ([`Cell::surface_material`]), and persistent [`Effects`] (water/ice/lava).
//! Shape is *not* stored — height is a bake-time function of the ledger.
//!
//! # The invariants this schema is built to honor
//!
//! - **Mass per element is the conserved truth** (system spec §4). Compositions
//!   support only conservation-safe arithmetic: [`Composition::add`] deposits,
//!   [`Composition::remove`] takes *at most* what is present and reports what it
//!   actually took. A player digging and a river eroding are the same call.
//! - **Surface-only, sparse** (handoff §4). The [`Ledger`] stores only
//!   materialized columns; everything below the surface is the 100%-solid bulk,
//!   revealed on dig. Absent coordinate = not yet materialized.
//! - **Pass-based, inert between sweeps** (system spec §5). Nothing here ticks;
//!   it is read as a snapshot and rewritten by the (deferred) erosion sweep.
//!
//! # Deliberately deferred (do not build here yet)
//!
//! The erosion sweep, the Rivulet flow structures, the degeneration/write-back
//! routine (spec §5-6), and the classifier that fills [`Cell::surface_material`]
//! (handoff §6). This crate supplies the substrate they will operate on.
//!
//! [`MaterialId`]: flicker_materials::MaterialId

pub mod cell;
pub mod composition;
pub mod ledger;

pub use cell::{Cell, Effects};
pub use composition::Composition;
pub use ledger::{CellCoord, Ledger};
