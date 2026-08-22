//! **The pixel tier** — where a share of a hex becomes a place.
//!
//! Up to this point the world's truth is the **aggregate**: one column of strata
//! per cell, ~92,000 of them, tens of megabytes for a planet. That is what the
//! whole geology runs on, and it is why it can run on a laptop. But an aggregate
//! has no *inside*: a cell knows it holds so much basalt, not where in itself the
//! basalt is, and you cannot walk around in a number.
//!
//! The **truth migration** is where that changes. Each cell's aggregate is
//! distributed into 2048² maps — one per stratum — and **from that moment the
//! pixels are the truth and the aggregate is a rollup of them.** It is a one-way
//! door, and the thing that walks through it is authority: `Σ(pixels) ≡ aggregate`
//! stops being a way to derive the pixels and becomes the trial balance that checks
//! them.
//!
//! # Why it has to be a migration and not a formula
//!
//! Because **where a thing is, is the payload**. A lead body 7,000 clusters across
//! is a spatial fact the earlier phases produced; you cannot recover it by dividing
//! a hex's lead by its pixel count, and a system that thinks it can will erase every
//! feature in the world the first time it tries. So this runs **once**, its output
//! is irreplaceable, and nothing downstream may reconstruct a pixel from the average
//! around it.
//!
//! # The three properties, and why each one is hard
//!
//! **Conservative.** Every gram the column held is in the maps and no gram is
//! invented. Not approximately — the audit that has caught every leak in this
//! simulation still has to pass on the other side of the door.
//!
//! **Continuous across a seam.** Two tiles meeting at a cell boundary must agree
//! about the ground there, or the world has a cliff around every hex. This is
//! achieved by construction rather than by matching: the surface is a **pure
//! function of position on the sphere** ([`shape::relief_at`]), so a tile does not
//! get a say in what the ground does at its own edge. Two tiles asking about the
//! same place get the same answer because they are asking the same question.
//!
//! **Deterministic.** Same world, same seed, same maps — so a tile that is thrown
//! away can be made again, byte for byte.
//!
//! The tension is between the first two: a field defined globally does not
//! integrate to exactly the column's mass over one tile, and rescaling the tile to
//! fix that would move its edges. So the correction is applied **only where no
//! other tile can see it** — weighted by how far a pixel is from the boundary, so
//! it is zero at the seam and absorbs the whole residual in the interior.
//!
//! # What a pixel is
//!
//! One pixel is **one voxel cluster** — 128 ft across, the same 128 ft the whole
//! scale chain is built on, and the unit the erosion sim will run per-pixel from
//! T7 onward. 2048 of them span a hex's 49.65 mi. About 3.8M of a tile's 4.19M
//! pixels fall inside the hexagon; the corners belong to nobody.

pub mod erosion;
pub mod mask;
pub mod materialize;
pub mod shape;

pub use erosion::{tile_mass_kg, BedProps, Eroder, ErosionParams, PassReport};
pub use mask::{HexMask, TileFrame, PIXELS_PER_TILE, TILE_DIM, TILE_SPAN_M};
pub use materialize::{materialize, Tile};

/// Feet across one pixel — the width of a voxel cluster. Together with
/// [`TILE_DIM`] and [`METRES_PER_FOOT`] it derives the canonical
/// [`TILE_SPAN_M`], which lives with the world constants in
/// `flicker-poc-chemistry` — the guard test below pins the two statements equal.
pub const FEET_PER_PIXEL: f64 = 128.0;

/// Metres per foot, so the scale chain is stated once.
pub const METRES_PER_FOOT: f64 = 0.3048;

/// **The planet radius a given grid frequency implies, m** — the freq↔radius
/// pinning the topology spec asks for, re-exported from the world constants
/// (`flicker-poc-chemistry::config`), where the whole size chain lives since
/// the two-models incident (2026-08-06): one derivation, both tiers.
pub use flicker_poc_chemistry::radius_for_freq;

#[cfg(test)]
mod scale_chain {
    /// The pixel tier's own statement of the span (2048 px × 128 ft × 0.3048)
    /// must be THE span the world constants carry — the drift between two
    /// derivations of this number is exactly how one repo ended up simulating
    /// two different-sized planets at once.
    #[test]
    fn the_tile_span_is_the_world_span() {
        let ours = super::TILE_DIM as f64 * super::FEET_PER_PIXEL * super::METRES_PER_FOOT;
        assert_eq!(
            ours,
            flicker_poc_chemistry::TILE_SPAN_M,
            "one span, one canon"
        );
    }
}
