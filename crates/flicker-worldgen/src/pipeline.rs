//! The epoch chain — Epoch 1 → 2 → … → 6, each reading the layer below it.
//!
//! Each epoch transforms the previous epoch's per-hex layer (world-gen spec:
//! Epoch 2 differentiation, 3 tectonics, 4 hydrosphere, 5 mineralization, 6
//! erosion/biomes). Until those transforms are written, every epoch is a
//! [`PassThrough`] that copies the layer below verbatim, so the stack shows
//! identical planes that will diverge into the geological progression as real
//! transforms replace the pass-throughs one at a time.
//!
//! The chain keeps **every** intermediate layer (not just the final one) so the
//! visualization can stack them — Epoch 1 at the bottom, Epoch 6 (the ground the
//! surface reads) at the top.

use flicker_worldstate::Composition;
use glam::Vec3;

use crate::epoch1::Epoch1;

/// One epoch's transform: read the previous epoch's per-hex layer, produce this
/// epoch's. (Epoch 1 is the seed — [`Epoch1`] — and is not a transform.)
pub trait EpochTransform {
    /// This epoch's number (2..=6), for labeling.
    fn epoch(&self) -> u8;
    /// Transform the previous layer (`prev`, one [`Composition`] per hex) into
    /// this epoch's layer, also one per hex.
    fn apply(&self, prev: &[Composition]) -> Vec<Composition>;
}

/// Placeholder transform — copies the previous layer verbatim. Stands in for an
/// epoch whose real geology isn't written yet (Epochs 2-6 today). Replacing one
/// of these with a real transform is the unit of future epoch work.
pub struct PassThrough(pub u8);

impl EpochTransform for PassThrough {
    fn epoch(&self) -> u8 {
        self.0
    }
    fn apply(&self, prev: &[Composition]) -> Vec<Composition> {
        prev.to_vec()
    }
}

/// Run the epoch chain over the hex directions `dirs`: layer 0 is Epoch 1's
/// seeded composition, then one layer per transform. The result is
/// `transforms.len() + 1` per-hex layers, bottom (Epoch 1) first.
pub fn epoch_stack(
    epoch1: &Epoch1,
    dirs: &[Vec3],
    transforms: &[&dyn EpochTransform],
) -> Vec<Vec<Composition>> {
    let mut layers = vec![epoch1.seed_world(dirs.iter().copied())];
    for t in transforms {
        let next = t.apply(layers.last().expect("at least the seed layer"));
        layers.push(next);
    }
    layers
}

/// The default six-epoch stack: Epoch 1 (seed) + Epochs 2-6 (pass-through copies
/// for now). Returns six per-hex layers — Epoch 1 at index 0, **Epoch 6 (the
/// ground) at index 5**.
pub fn six_epoch_stack(epoch1: &Epoch1, dirs: &[Vec3]) -> Vec<Vec<Composition>> {
    let (p2, p3, p4, p5, p6) = (
        PassThrough(2),
        PassThrough(3),
        PassThrough(4),
        PassThrough(5),
        PassThrough(6),
    );
    let transforms: [&dyn EpochTransform; 5] = [&p2, &p3, &p4, &p5, &p6];
    epoch_stack(epoch1, dirs, &transforms)
}

/// Number of epochs in the default stack ([`six_epoch_stack`]); the ground is
/// the last (`EPOCHS - 1`).
pub const EPOCHS: usize = 6;

#[cfg(test)]
mod tests {
    use super::*;
    use flicker_materials::{JsonTableSource, Tables};

    use crate::epoch1::Epoch1Params;

    fn tables() -> Tables {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../data/materials");
        Tables::from_source(&JsonTableSource::new(dir)).expect("repo data/materials loads")
    }

    fn dirs() -> Vec<Vec3> {
        // A handful of arbitrary unit directions standing in for hexes.
        (0..12)
            .map(|i| {
                let a = i as f32 / 12.0 * std::f32::consts::TAU;
                Vec3::new(a.cos(), (i as f32 * 0.13).sin(), a.sin()).normalize()
            })
            .collect()
    }

    #[test]
    fn six_layers_ground_is_last() {
        let t = tables();
        let e1 = Epoch1::new(&t, Epoch1Params::default(), 7);
        let d = dirs();
        let stack = six_epoch_stack(&e1, &d);
        assert_eq!(stack.len(), EPOCHS, "six epoch layers");
        for layer in &stack {
            assert_eq!(layer.len(), d.len(), "one composition per hex");
        }
    }

    #[test]
    fn passthroughs_copy_the_layer_below() {
        let t = tables();
        let e1 = Epoch1::new(&t, Epoch1Params::default(), 7);
        let d = dirs();
        let stack = six_epoch_stack(&e1, &d);
        // Today every epoch is a copy: all six layers equal Epoch 1.
        for layer in &stack[1..] {
            assert_eq!(layer, &stack[0], "pass-through layer should copy Epoch 1");
        }
        // And layer 0 is exactly the Epoch 1 seeding.
        assert_eq!(stack[0], e1.seed_world(d.iter().copied()));
    }

    #[test]
    fn a_real_transform_diverges_from_the_copies() {
        // Prove the chain threads data layer-to-layer: a transform that mutates
        // its input makes everything above it differ. (Stand-in for a future
        // real epoch.)
        struct AddCarbon;
        impl EpochTransform for AddCarbon {
            fn epoch(&self) -> u8 {
                2
            }
            fn apply(&self, prev: &[Composition]) -> Vec<Composition> {
                prev.iter()
                    .map(|c| {
                        let mut c = c.clone();
                        c.add(6, 1.0); // carbon
                        c
                    })
                    .collect()
            }
        }
        let t = tables();
        let e1 = Epoch1::new(&t, Epoch1Params::default(), 7);
        let d = dirs();
        let add = AddCarbon;
        let pass = PassThrough(3);
        let transforms: [&dyn EpochTransform; 2] = [&add, &pass];
        let stack = epoch_stack(&e1, &d, &transforms);
        assert_ne!(stack[1], stack[0], "transform changed the layer");
        assert_eq!(stack[2], stack[1], "pass-through copied the transformed layer");
    }
}
