//! flicker-worldgen — the offline **world-generation** pipeline (system spec
//! §13 "world-gen").
//!
//! The epoch kernels that turn a seed + parameters into the foundational
//! composition maps the runtime material model ([`flicker_worldstate`], tier ②)
//! runs on — "the gap from zero to a planet that looks like a planet" (material
//! handoff §7). Offline-heavy; the runtime links none of it.
//!
//! This crate currently implements **Epoch 1** only — initial per-hex element
//! distribution ([`Epoch1`]). Epochs 2-6 (differentiation, tectonics,
//! hydrosphere, mineralization, erosion/biomes) are deferred.
//!
//! Output is a [`Composition`] per hex; wrapping it into a ledger [`Cell`] /
//! [`Ledger`] is the caller's (seam) concern.
//!
//! [`Composition`]: flicker_worldstate::Composition
//! [`Cell`]: flicker_worldstate::Cell
//! [`Ledger`]: flicker_worldstate::Ledger

pub mod epoch1;
pub mod noise;

pub use epoch1::{Epoch1, Epoch1Params};

use std::collections::BTreeMap;

use flicker_materials::ElementId;
use flicker_worldstate::Composition;

/// Count how many compositions have each element as their dominant one — a
/// headless "see and verify" summary of a generated distribution (the verifiable
/// face of the dominant-element tint, before any GPU render).
pub fn dominant_histogram(comps: &[Composition]) -> BTreeMap<ElementId, usize> {
    let mut hist = BTreeMap::new();
    for c in comps {
        if let Some(id) = c.dominant() {
            *hist.entry(id).or_insert(0) += 1;
        }
    }
    hist
}
