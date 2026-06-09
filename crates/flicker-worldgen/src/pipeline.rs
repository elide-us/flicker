//! The epoch chain — Epoch 1 → 2 → … → 6, each reading the layer below it and
//! adding/transforming per-hex fields ([`HexState`]).
//!
//! Epoch 1 seeds the bulk composition; **Epoch 2** differentiates a crust;
//! **Epoch 3** lays down plates and elevation (continents / mountains). Epochs
//! 4-6 are [`PassThrough`] copies until their transforms (hydrosphere,
//! mineralization, erosion / biomes) are written — replacing one is the unit of
//! future epoch work. The chain keeps **every** layer so the stack can be
//! visualized epoch by epoch.

use flicker_materials::Tables;
use glam::Vec3;

use crate::epoch1::Epoch1;
use crate::epoch2::Epoch2;
use crate::epoch3::Epoch3;
use crate::state::HexState;

/// Number of epochs in the default stack ([`six_epoch_stack`]); the ground is
/// the last (`EPOCHS - 1`).
pub const EPOCHS: usize = 6;

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
/// Epoch 3 (tectonics) → Epochs 4-6 (pass-through). Returns six per-hex layers,
/// **Epoch 1 at index 0, Epoch 6 (the ground) at index 5**.
pub fn six_epoch_stack(epoch1: &Epoch1, ctx: &EpochCtx) -> Vec<Vec<HexState>> {
    let seed_layer: Vec<HexState> = ctx
        .dirs
        .iter()
        .map(|&d| HexState::new(epoch1.seed_hex(d)))
        .collect();
    let (e2, e3) = (Epoch2::default(), Epoch3::default());
    let (p4, p5, p6) = (PassThrough(4), PassThrough(5), PassThrough(6));
    let transforms: [&dyn EpochTransform; 5] = [&e2, &e3, &p4, &p5, &p6];
    epoch_stack(seed_layer, ctx, &transforms)
}

#[cfg(test)]
mod tests {
    use super::*;
    use flicker_materials::{JsonTableSource, Tables};

    use crate::epoch1::Epoch1Params;
    use crate::state::Boundary;

    fn tables() -> Tables {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../data/materials");
        Tables::from_source(&JsonTableSource::new(dir)).expect("repo data/materials loads")
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
        // Epochs 4-6 are pass-through copies of Epoch 3.
        assert_eq!(stack[3], stack[2]);
        assert_eq!(stack[4], stack[2]);
        assert_eq!(stack[5], stack[2]);
    }
}
