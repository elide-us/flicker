//! Planet topology substrate — the icosahedral hex grid (docs/hex-sphere-handoff.md).
//!
//! This crate produces *topology only*: per-cell position (`dir`), the variable-
//! length neighbour list (5 for a pentagon defect, 6 for an interior hex), area
//! and a pentagon flag. Those are exactly what the world-gen pipeline already
//! consumes — `EpochCtx { dirs: &[Vec3], neighbors: &[Vec<u32>], .. }` — so a
//! pentagon needs no special-casing downstream; it is simply a cell whose
//! neighbour vec has length five.
//!
//! **Slice 1** ([`pentagon_patch`]) builds a single pentagon-centered patch: the
//! cap of five icosahedron faces meeting at one vertex, geodesically subdivided
//! and dualised. That is the minimal *complete* hard case — one defect, its full
//! five-fold neighbourhood, and the seams radiating from it — small enough to
//! run on a weak box.
//!
//! **Slice 3** ([`icosphere`]) builds the whole planet: all twenty faces
//! subdivided into one closed grid with the twelve pentagons spread over the
//! icosahedron vertices, each cell carrying its shard and a stable [`CellId`].
//!
//! **Slice 3b** (`isea`) places every point by the **Snyder equal-area map**, so
//! a cell covers the same area wherever it sits. That is what makes the ledgers'
//! absolute element masses comparable across the planet, and what makes "one hex
//! step" a single distance for the tectonic conveyor. Adjacency is independent of
//! the projection, so this moved points without re-deriving the graph — and the
//! ledger-key id layout is still a later slice for the same reason.

mod isea;
mod mesh;
mod patch;
mod sphere;

pub use patch::{pentagon_patch, Patch};
pub use sphere::{cell_count, icosphere, icosphere_with_outlines, CellId, Sphere};
