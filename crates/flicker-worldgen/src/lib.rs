//! flicker-worldgen — the offline **world-generation** pipeline (system spec
//! §13 "world-gen").
//!
//! The epoch kernels that turn a seed + parameters into the foundational
//! composition maps the runtime material model ([`flicker_worldstate`], tier ②)
//! runs on — "the gap from zero to a planet that looks like a planet" (material
//! handoff §7). Offline-heavy; the runtime links none of it.
//!
//! The chain so far ([`HexState`] threads through it):
//! - **Epoch 1** ([`Epoch1`]) — initial per-hex element distribution.
//! - **Epoch 2** ([`Epoch2`]) — differentiation: light silicates rise to a
//!   crust, heavy metals sink.
//! - **Epoch 3** ([`Epoch3`]) — plate tectonics: plates, boundaries, and the
//!   mean-surface-**elevation** that gives continents and mountains.
//! - **Epoch 4** ([`Epoch4`]) — hydrosphere: oceans fill the low basins; surface
//!   temperature from latitude + elevation lapse.
//! - **Epoch 5** ([`Epoch5`]) — mineralization: hydrothermal ore veins
//!   concentrate metal along the fault network.
//! - **Epoch 6** ([`Epoch6`]) — erosion / biomes: macro-erode the terrain into a
//!   starting landscape, route drainage, and classify each hex's biome.
//!
//! [`Composition`]: flicker_worldstate::Composition

pub mod epoch1;
pub mod epoch2;
pub mod epoch3;
pub mod epoch4;
pub mod epoch5;
pub mod epoch6;
pub mod field;
pub mod noise;
pub mod pipeline;
pub mod state;

pub use epoch1::{Epoch1, Epoch1Params};
pub use epoch2::Epoch2;
pub use epoch3::{Epoch3, Partition, Plate};
pub use epoch4::Epoch4;
pub use epoch5::Epoch5;
pub use epoch6::{watersheds, Epoch6, Watershed};
pub use field::{CellSample, FieldSampler};
pub use pipeline::{epoch_stack, six_epoch_stack, EpochCtx, EpochTransform, PassThrough, EPOCHS};
pub use state::{Biome, Boundary, HexState, LifeStage};

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
