//! Per-cell **spatial fields** sampled from a hex's epoch [`HexState`].
//!
//! The epochs produce per-hex *aggregates*; this turns them into the continuous
//! sub-hex fields that give terrain its structure. The spine is the **hardness
//! field**: different elements sit in different micro-regions (independent NS
//! sub-fields per element — the "resonances"), so the blended hardness varies
//! across the hex. Hard rock stands up into ridges/mountains, soft rock is planed
//! into valleys/seabeds — the `elevation` relief this returns. That hardness
//! distribution is what the water-cycle erosion later carves.
//!
//! Before the fields are read, the sample point is **convected** — advected along
//! an iterated low-frequency flow, the molten stirring ("swirling iron") that
//! froze into the crust — so the structure reads as laminar swirls and bands
//! rather than isotropic blobs. (Kinematic flow for now; buoyancy-driven dynamics
//! are a later deepening.)
//!
//! Once a hex's crust has set (Epoch 2+) the relief is **ridged** — sharp crests,
//! like grain boundaries and fold ranges — rather than the smooth swell of the
//! melt: the crystallization step. Hardness blends only the **solid** formers
//! (gases are binders, not hardness-bearers, and their zero element-hardness
//! would wash the blend out), so silica-rich rock stands hard and varies cell to
//! cell. Where plates collide (Epoch 3 `orogeny`), compression folds finer ridges
//! in and lifts the belt — mountain ranges along the convergent boundary.
//!
//! Fields are sampled in a continuous world coordinate (cluster units), so
//! adjacent hexes that sample shared edge positions agree — terrain flows across
//! hex boundaries with no seam.

use flicker_materials::{ElementId, PhysicalState, Tables};
use glam::{Vec2, Vec3};

use crate::noise::fbm;
use crate::state::HexState;

/// What a single cell resolves to.
pub struct CellSample {
    /// Blended Mohs-like hardness of the per-cell composition, `0..10`.
    pub hardness: f32,
    /// Surface relief at this cell, world units: the hex's tectonic base
    /// elevation plus a hardness-weighted ridge field.
    pub elevation: f32,
    /// The locally dominant element (for a composition tint).
    pub dominant: Option<ElementId>,
}

/// Samples the per-cell fields of a hex. Cheap to build (just borrows the
/// vocabulary + seed); one per render pass.
pub struct FieldSampler<'a> {
    tables: &'a Tables,
    seed: u64,
    /// Spatial frequency of the per-element composition sub-fields.
    pub composition_freq: f32,
    /// Spatial frequency of the relief ridges.
    pub relief_freq: f32,
    /// Ridge amplitude (world units) at full hardness.
    pub relief_amp: f32,
    /// Tectonic elevation (`HexState::elevation`, `-1..1`) → world units.
    pub tectonic_scale: f32,
    /// Octaves for both fields.
    pub octaves: u32,
    /// Floor of an element's per-cell weight where its sub-field is `0`.
    pub floor: f64,
    /// Skip elements below this fraction of the column total (trace metals don't
    /// move the hardness blend, and skipping them keeps sampling cheap).
    pub trace_cutoff: f64,
    /// Convection passes — the molten flow stirred the composition this many
    /// times before the crust froze. `0` = static fields (no convection).
    pub convection_iters: u32,
    /// Spatial frequency of the convective flow (lower = broader swirls).
    pub flow_freq: f32,
    /// World-unit displacement per convection pass.
    pub flow_strength: f32,
    /// Extra relief amplitude at a full-strength orogenic belt (multiplier).
    pub orogeny_lift: f32,
    /// Fold-ridge frequency at orogenic belts, relative to the base relief.
    pub fold_freq_mult: f32,
}

impl<'a> FieldSampler<'a> {
    /// A sampler with display-tuned defaults.
    pub fn new(tables: &'a Tables, seed: u64) -> Self {
        Self {
            tables,
            seed,
            composition_freq: 0.003,
            relief_freq: 0.002,
            relief_amp: 80.0,
            tectonic_scale: 120.0,
            octaves: 3,
            floor: 0.2,
            trace_cutoff: 0.01,
            convection_iters: 3,
            flow_freq: 0.0008,
            flow_strength: 250.0,
            orogeny_lift: 1.8,
            fold_freq_mult: 5.0,
        }
    }

    /// Advect a sample point along an iterated low-frequency flow — the molten
    /// convection that stirred the composition into laminar swirls before the
    /// crust froze. Same domain-warp idiom as the heightmap field, iterated.
    fn convect(&self, mut p: Vec3) -> Vec3 {
        for i in 0..self.convection_iters {
            let s = 0x0F10_0000 ^ i as u64;
            let fx = fbm(p * self.flow_freq, 2, s, self.seed) * 2.0 - 1.0;
            let fz = fbm(p * self.flow_freq, 2, s ^ 0x9999, self.seed) * 2.0 - 1.0;
            p += Vec3::new(fx as f32, 0.0, fz as f32) * self.flow_strength;
        }
        p
    }

    /// Sample the cell at continuous world position `world` (cluster units) for a
    /// hex in state `state`.
    pub fn sample(&self, state: &HexState, world: Vec2) -> CellSample {
        let p = self.convect(Vec3::new(world.x, 0.0, world.y));
        let surf = state.surface();
        let cutoff = surf.total() * self.trace_cutoff;

        // Per-element NS sub-fields → per-cell composition → blended hardness.
        let (mut h_sum, mut w_sum) = (0.0f64, 0.0f64);
        let mut best: (Option<ElementId>, f64) = (None, f64::MIN);
        for (el, amount) in surf.iter() {
            if amount < cutoff {
                continue;
            }
            let n = fbm(p * self.composition_freq, self.octaves, el as u64 ^ 0x00C0_FFEE, self.seed);
            let local = amount * (self.floor + (1.0 - self.floor) * n);
            if let Some(e) = self.tables.element_by_number(el) {
                // Gases are binders, not hardness-bearers — skip them so the blend
                // reflects the solid rock-formers (and isn't dragged to ~0 by O).
                if e.state != PhysicalState::Gas {
                    h_sum += local * e.hardness as f64;
                    w_sum += local;
                }
            }
            if local > best.1 {
                best = (Some(el), local);
            }
        }
        let hardness = if w_sum > 0.0 { (h_sum / w_sum) as f32 } else { 0.0 };

        // Relief: tectonic base + a hardness-weighted ridge. Soft rock barely
        // rises; hard rock throws up the full ridge amplitude. Once the crust has
        // crystallized (Epoch 2+) the field is **ridged** into sharp crests; the
        // molten phase (Epoch 1) is a smooth swell.
        let raw = fbm(p * self.relief_freq, self.octaves, 0x0002_1D9E, self.seed);
        let mut ridge = if state.crust.is_empty() {
            ((raw - 0.5) * 2.0) as f32
        } else {
            ((1.0 - 2.0 * (raw - 0.5).abs()) * 2.0 - 1.0) as f32
        };
        // Orogeny: where plates collide, compression folds finer ridges in and
        // lifts the belt higher — mountain ranges along the convergent boundary.
        let mut amp = self.relief_amp;
        if state.orogeny > 0.0 {
            let f = fbm(p * self.relief_freq * self.fold_freq_mult, self.octaves, 0x000F_01D5, self.seed);
            let fold = ((1.0 - 2.0 * (f - 0.5).abs()) * 2.0 - 1.0) as f32;
            ridge += state.orogeny * 0.6 * fold;
            amp *= 1.0 + state.orogeny * self.orogeny_lift;
        }
        let hard_norm = (hardness / 10.0).clamp(0.0, 1.0);
        let elevation = state.elevation * self.tectonic_scale + amp * ridge * (0.3 + 0.7 * hard_norm);

        CellSample { hardness, elevation, dominant: best.0 }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flicker_materials::JsonTableSource;
    use flicker_worldstate::Composition;

    const SI: ElementId = 14;
    const FE: ElementId = 26;

    fn tables() -> Tables {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../data/materials");
        Tables::from_source(&JsonTableSource::new(dir)).expect("repo data/materials loads")
    }

    #[test]
    fn hardness_varies_across_space_and_is_in_range() {
        let t = tables();
        let s = FieldSampler::new(&t, 7);
        // A mixed iron/silicon column.
        let state = HexState::new(Composition::from_iter([(FE, 5000.0), (SI, 5000.0)]));
        let a = s.sample(&state, Vec2::new(0.0, 0.0));
        let b = s.sample(&state, Vec2::new(4000.0, -2500.0));
        assert!((0.0..=10.0).contains(&a.hardness));
        assert!((0.0..=10.0).contains(&b.hardness));
        assert!((a.hardness - b.hardness).abs() > 1e-3, "hardness field is flat");
        assert!(a.elevation.is_finite() && b.elevation.is_finite());
    }

    #[test]
    fn convection_warps_the_field() {
        let t = tables();
        let mut still = FieldSampler::new(&t, 7);
        still.convection_iters = 0;
        let flowed = FieldSampler::new(&t, 7); // default convection on
        let state = HexState::new(Composition::from_iter([(FE, 4000.0), (SI, 6000.0)]));
        // Convection moves where the field reads, so the two disagree in places.
        let differ = (0..50)
            .filter(|&k| {
                let w = Vec2::new(k as f32 * 137.0, k as f32 * 91.0);
                (still.sample(&state, w).hardness - flowed.sample(&state, w).hardness).abs() > 1e-3
            })
            .count();
        assert!(differ > 5, "convection didn't change the field ({differ}/50)");
    }

    #[test]
    fn crust_crystallizes_into_ridges() {
        let t = tables();
        let s = FieldSampler::new(&t, 7);
        let comp = Composition::from_iter([(SI, 8000.0), (FE, 2000.0)]);
        let molten = HexState::new(comp.clone()); // crust empty → smooth melt
        let mut crust = HexState::new(comp.clone());
        crust.crust = comp; // crust set → crystallized (ridged) relief
        let differ = (0..50)
            .filter(|&k| {
                let w = Vec2::new(k as f32 * 70.0, k as f32 * 110.0);
                (s.sample(&molten, w).elevation - s.sample(&crust, w).elevation).abs() > 1e-3
            })
            .count();
        assert!(differ > 5, "crystallization didn't reshape the relief ({differ}/50)");
    }

    #[test]
    fn orogeny_lifts_convergent_belts() {
        let t = tables();
        let s = FieldSampler::new(&t, 7);
        let comp = Composition::from_iter([(SI, 8000.0), (FE, 2000.0)]);
        let mut flat = HexState::new(comp.clone());
        flat.crust = comp.clone(); // crystallized crust, no orogeny
        let mut belt = HexState::new(comp.clone());
        belt.crust = comp;
        belt.orogeny = 1.0; // full fold belt
        // The belt's relief should swing markedly higher than the un-folded crust.
        let span = |st: &HexState| {
            let (mut lo, mut hi) = (f32::MAX, f32::MIN);
            for k in 0..60 {
                let e = s.sample(st, Vec2::new(k as f32 * 60.0, k as f32 * 80.0)).elevation;
                lo = lo.min(e);
                hi = hi.max(e);
            }
            hi - lo
        };
        assert!(span(&belt) > span(&flat) * 1.3, "orogeny didn't amplify the relief");
    }

    #[test]
    fn deterministic_for_a_position() {
        let t = tables();
        let s = FieldSampler::new(&t, 7);
        let state = HexState::new(Composition::from_iter([(SI, 1000.0)]));
        let a = s.sample(&state, Vec2::new(123.0, 456.0));
        let b = s.sample(&state, Vec2::new(123.0, 456.0));
        assert_eq!(a.hardness.to_bits(), b.hardness.to_bits());
        assert_eq!(a.elevation.to_bits(), b.elevation.to_bits());
    }

    #[test]
    fn tectonic_elevation_lifts_the_whole_cell() {
        let t = tables();
        let s = FieldSampler::new(&t, 7);
        let comp = Composition::from_iter([(SI, 1000.0)]);
        let mut low = HexState::new(comp.clone());
        low.elevation = -0.5;
        let mut high = HexState::new(comp);
        high.elevation = 0.5;
        let at = Vec2::new(200.0, 200.0);
        // Same position, higher tectonic base ⇒ higher cell elevation.
        assert!(s.sample(&high, at).elevation > s.sample(&low, at).elevation);
    }
}
