//! The celestial **data model** — a star system as a recursive tree of bodies and
//! satellites, carrying the four physical fields (composition, gravity, density,
//! pressure). Data only: formation and evolution (and the renderers + epoch sim)
//! *consume* these types; nothing here knows about them.
//!
//! - [`condensation`] — the [`CondensationClass`] / [`ClassComposition`] physics
//!   view of what a body is made of (the bulk-density truth) and its exact bridge
//!   to the conserved element ledger.
//! - [`cloud`] — [`Cloud`], the system's material reservoir as pure conservation
//!   accounting; bodies absorb their mass from it.
//! - [`body`] — [`Body`], the recursive node, with the four fields derived from its
//!   composition; [`BodyKind`], its intrinsic formation type.
//! - [`satellite`] — [`Satellite`] (a child [`Body`] or a [`Disc`]) and the
//!   ring↔belt [`Disc`] continuum.
//! - [`system`] — [`System`], the tree wrapper, with walks and IAU
//!   [`Classification`].

pub mod body;
pub mod cloud;
pub mod condensation;
pub mod satellite;
pub mod system;

pub use body::{Body, BodyKind};
pub use cloud::{Cloud, CloudRing};
pub use condensation::{ClassComposition, CondensationClass};
pub use satellite::{Disc, DiscClass, DiscGap, Satellite, RING_BELT_SURFACE_DENSITY};
pub use system::{classify, cleared_neighborhood, Classification, System, COMET_MIN_ECCENTRICITY};
