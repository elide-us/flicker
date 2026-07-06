//! flicker-celestial — the unified celestial simulation crate.
//!
//! This crate consolidates the celestial logic that grew scattered across several
//! POCs (the `flicker-solarsystem` formation sim, `flicker-world`'s day/night sun,
//! `voxel-cluster`'s original sky model) into **one place**, and pulls the *logic*
//! away from the *data* (see `docs/flicker-celestial-spec.md`). It serves two
//! co-equal objectives: **effective visualizations** *and* **accurate data**.
//!
//! # Layering (dependencies point down only)
//!
//! ```text
//! flicker-celestial
//! ├── model/      data only: Body, Satellite (tree), System, Cloud, HexWorld, the four fields
//! ├── formation/  nebula → conserved Cloud (materialisation done; seeding next)
//! └── evolution/  iterate a body's HexWorld over time (skeleton: lifecycle + step seam)
//! ```
//!
//! `model` knows nothing of formation/evolution; those (and the renderers + epoch
//! sim, which live *outside* this crate) consume the model. This crate has **no GPU
//! dependency** — rendering is a consumer's job.
//!
//! # What's built
//!
//! [`model`] — the data:
//!
//! - [`Body`](model::Body) — a physical object carrying the four fields:
//!   **composition** (the conserved element-mass truth) and the **gravity /
//!   density / pressure** that derive from it.
//! - [`Satellite`](model::Satellite) — the recursive tree edge: a bound child
//!   [`Body`](model::Body) (moon / submoon / comet) or a [`Disc`](model::Disc) of
//!   material (ring / belt, a single type on a density continuum).
//! - [`System`](model::System) — a whole star system: the star as the root node,
//!   with tree walks and IAU classification over it.
//! - [`Cloud`](model::Cloud) — the system's material reservoir, as pure conservation
//!   accounting (no visual): bodies grow by absorbing its mass (removed from the
//!   cloud, inserted into the body), leaving a conserved remainder.
//! - [`HexWorld`](model::HexWorld) — a body's surface as a hex grid of conserved
//!   composition (the planet-scale macro-voxel, §8): the world-storage foundation.
//!
//! [`formation`] materialises a nebula into the conserved [`Cloud`]. [`evolution`] iterates a
//! body's [`HexWorld`] over a mega-year lifecycle ([`BodyEvolution`]) — the world-gen epochs run
//! *per body and stepped*, with emergent stage transitions (no per-type terminations). Both are
//! early: formation has the materialisation (seeding next); evolution has the lifecycle skeleton
//! + step seam (the per-stage transforms next). See `docs/flicker-celestial-evolution-handoff.md`.
//!
//! # Units
//!
//! Orbital mechanics use the formation convention — **AU**, **years**, **solar
//! masses**, with `G = 4π²` (a 1 AU orbit around 1 M☉ takes one year). Physical
//! surface quantities are reported in SI/CGS with explicit unit suffixes
//! (`_si` = m/s², g/cm³, GPa). See [`units`].

pub mod evolution;
pub mod formation;
pub mod hex;
pub mod model;
pub mod units;

pub use evolution::{BodyEvolution, Stage};
pub use model::{
    Body, BodyKind, ClassComposition, Classification, Cloud, CloudRing, CondensationClass, Disc,
    DiscClass, DiscGap, HexWorld, Satellite, System,
};
