//! The epoch chain — Epoch 1 → 2 → … → 6, each reading the layer below it and
//! adding/transforming per-hex fields ([`HexState`]).
//!
//! Epoch 1 seeds the bulk composition; **Epoch 2** differentiates a crust;
//! **Epoch 3** lays down plates and elevation (continents / mountains); **Epoch
//! 4** floods the basins (hydrosphere); **Epoch 5** mineralizes ore veins along
//! the faults; **Epoch 6** erodes the terrain into a starting landscape and
//! assigns biomes. The chain keeps **every** layer so the stack can be
//! visualized epoch by epoch; [`PassThrough`] stands in for any epoch whose real
//! transform isn't written yet.

use flicker_materials::Tables;
use glam::Vec3;

use crate::epoch1::Epoch1;
use crate::epoch2::Epoch2;
use crate::epoch3::Epoch3;
use crate::epoch4::Epoch4;
use crate::epoch5::Epoch5;
use crate::epoch6::Epoch6;
use crate::state::HexState;

/// Number of epochs in the default stack ([`six_epoch_stack`]); the ground is
/// the last (`EPOCHS - 1`).
pub const EPOCHS: usize = 6;

/// The **nominal** value of each epoch's `duration` knob on the shared relative
/// "cycle" clock. Every post-snapshot epoch (2–6) centres its duration here, and
/// at this value reproduces its baseline output; below it the phase is shorter
/// (less differentiation / drift / chemistry / mineralization / erosion), above it
/// longer. Epoch 1 is the snapshot the clock starts from and has no duration.
pub const NOMINAL_DURATION: u32 = 5;

/// Shared, read-only context the epoch transforms run against: the vocabulary,
/// each hex's unit-sphere direction, its neighbour indices (for the plate /
/// erosion epochs), and the world seed.
pub struct EpochCtx<'a> {
    pub tables: &'a Tables,
    pub dirs: &'a [Vec3],
    pub neighbors: &'a [Vec<u32>],
    pub seed: u64,
}

/// One epoch's transform: read the previous epoch's per-hex layer, produce this
/// epoch's. (Epoch 1 is the seed — [`Epoch1`] — not a transform.)
pub trait EpochTransform {
    /// This epoch's number (2..=6), for labeling.
    fn epoch(&self) -> u8;
    /// Transform the previous layer into this epoch's.
    fn apply(&self, ctx: &EpochCtx, prev: &[HexState]) -> Vec<HexState>;
}

/// Placeholder transform — copies the previous layer verbatim. Stands in for an
/// epoch whose real geology isn't written yet (Epochs 4-6 today).
pub struct PassThrough(pub u8);

impl EpochTransform for PassThrough {
    fn epoch(&self) -> u8 {
        self.0
    }
    fn apply(&self, _ctx: &EpochCtx, prev: &[HexState]) -> Vec<HexState> {
        prev.to_vec()
    }
}

/// Run the chain from a seed layer through `transforms`, keeping every layer:
/// result `[0]` is the seed, then one layer per transform.
pub fn epoch_stack(
    seed_layer: Vec<HexState>,
    ctx: &EpochCtx,
    transforms: &[&dyn EpochTransform],
) -> Vec<Vec<HexState>> {
    let mut layers = vec![seed_layer];
    for t in transforms {
        let next = t.apply(ctx, layers.last().expect("at least the seed layer"));
        layers.push(next);
    }
    layers
}

/// The default six-epoch stack: Epoch 1 (seed) → Epoch 2 (differentiation) →
/// Epoch 3 (tectonics) → Epoch 4 (hydrosphere) → Epoch 5 (mineralization) →
/// Epoch 6 (erosion / biomes). Returns six per-hex layers, **Epoch 1 at index 0,
/// Epoch 6 (the ground) at index 5**.
pub fn six_epoch_stack(epoch1: &Epoch1, ctx: &EpochCtx) -> Vec<Vec<HexState>> {
    let seed_layer: Vec<HexState> = ctx
        .dirs
        .iter()
        .map(|&d| HexState::new(epoch1.seed_hex(d)))
        .collect();
    let (e2, e3, e4) = (Epoch2::default(), Epoch3::default(), Epoch4::default());
    let (e5, e6) = (Epoch5::default(), Epoch6::default());
    let transforms: [&dyn EpochTransform; 5] = [&e2, &e3, &e4, &e5, &e6];
    epoch_stack(seed_layer, ctx, &transforms)
}

#[cfg(test)]
mod tests {
    use super::*;
    use flicker_materials::{JsonTableSource, Tables};

    use crate::epoch1::Epoch1Params;
    use crate::state::Boundary;

    fn tables() -> Tables {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../Alpha/content/data");
        Tables::from_source(&JsonTableSource::new(dir)).expect("repo Alpha/content/data loads")
    }

    /// A ring of `n` hexes around the equator — a minimal connected world.
    fn ring(n: usize) -> (Vec<Vec3>, Vec<Vec<u32>>) {
        let dirs = (0..n)
            .map(|i| {
                let a = i as f32 / n as f32 * std::f32::consts::TAU;
                Vec3::new(a.cos(), 0.0, a.sin())
            })
            .collect();
        let neighbors = (0..n)
            .map(|i| vec![((i + 1) % n) as u32, ((i + n - 1) % n) as u32])
            .collect();
        (dirs, neighbors)
    }

    #[test]
    fn six_layers_threaded_through_the_chain() {
        let t = tables();
        let e1 = Epoch1::new(&t, Epoch1Params::default(), 7);
        let (dirs, neighbors) = ring(30);
        let ctx = EpochCtx { tables: &t, dirs: &dirs, neighbors: &neighbors, seed: 7 };
        let stack = six_epoch_stack(&e1, &ctx);

        assert_eq!(stack.len(), EPOCHS);
        for layer in &stack {
            assert_eq!(layer.len(), dirs.len());
        }
        // Epoch 1 (seed): undifferentiated, sea-level.
        assert!(stack[0].iter().all(|s| s.crust.is_empty() && s.elevation == 0.0));
        // Epoch 2: a crust appeared.
        assert!(stack[1].iter().all(|s| !s.crust.is_empty()));
        // Epoch 3: plates + elevation written.
        assert!(stack[2].iter().any(|s| s.elevation != 0.0));
        assert!(stack[2].iter().any(|s| s.boundary != Boundary::Interior));
        // Epoch 4 adds the hydrosphere (sea level + oceans).
        assert_ne!(stack[3], stack[2], "Epoch 4 should change the state");
        assert!(stack[3].iter().any(|s| s.water_depth > 0.0), "Epoch 4 made no ocean");
        // Epoch 5 mineralizes: hydrothermal signature + ore veins appear.
        assert_ne!(stack[4], stack[3], "Epoch 5 should change the state");
        assert!(stack[4].iter().any(|s| s.hydrothermal > 0.0), "Epoch 5 made no hydrothermal activity");
        assert!(stack[4].iter().any(|s| s.vein_element.is_some()), "Epoch 5 formed no veins");
        // Epoch 6 erodes + assigns biomes.
        assert_ne!(stack[5], stack[4], "Epoch 6 should change the state");
        assert!(stack[5].iter().any(|s| s.flow > Epoch6::default().rain), "Epoch 6 routed no drainage");
    }
}
