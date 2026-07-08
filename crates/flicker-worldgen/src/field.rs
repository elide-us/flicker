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
//! Two Epoch-5 structures break the crust's uniformity into the **seams** the
//! erosion sim wants. A hex carrying an ore vein (`vein_element`) sees its metal
//! routed not through the smooth composition sub-field but a **filament** — thin
//! sinuous crests where the metal concentrates and the troughs between revert to
//! the host rock — so the vein reads as a banded streak that takes on the metal's
//! hardness. And the crystallized crust gains a **foliation**: anisotropic,
//! gently folded strata that modulate hardness up and down across the layering,
//! the differential a rivulet later carves into ribs and runnels.
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
    /// Ore-vein intensity at this cell, `0..1` (Epoch 5). `0` off a vein; rises
    /// on the thin filaments where the hex's `vein_element` concentrates. The
    /// overlay signal for the renderer, and why these cells erode differently.
    pub vein: f32,
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
    /// Spatial frequency of the ore-vein filaments (higher = thinner, denser
    /// bands).
    pub vein_freq: f32,
    /// Sharpness of a filament crest — the power the ridge is raised to. Higher
    /// pinches the vein into a thinner streak.
    pub vein_sharp: f64,
    /// Multiplier on the vein metal's local amount in a filament **trough**
    /// (`< 1` clears the metal out from between the bands).
    pub vein_low: f64,
    /// Multiplier on the vein metal's local amount on a filament **crest**
    /// (`> 1` concentrates it into the streak so it can dominate the cell).
    pub vein_gain: f64,
    /// Spatial frequency of the foliation strata (the metamorphic/sedimentary
    /// layering that bands the hardness).
    pub foliation_freq: f32,
    /// Fractional hardness swing across the foliation (`0.3` = ±30% between a
    /// hard layer and the soft layer beside it). `0` disables it.
    pub foliation_amp: f32,
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
            vein_freq: 0.004,
            vein_sharp: 3.0,
            vein_low: 0.12,
            vein_gain: 6.0,
            foliation_freq: 0.0035,
            foliation_amp: 0.3,
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

    /// A vein filament at `p`, `0..1`: a ridged field pinched into thin sinuous
    /// crests (`vein_sharp`) where an ore vein concentrates its metal, ~0 in the
    /// host rock between them.
    fn filament(&self, p: Vec3) -> f64 {
        let n = fbm(p * self.vein_freq, self.octaves, 0x5EED_0FA1, self.seed);
        let ridge = (1.0 - 2.0 * (n - 0.5).abs()).clamp(0.0, 1.0);
        ridge.powf(self.vein_sharp.max(1.0))
    }

    /// Foliation at `p`, `-1..1`: anisotropic, gently folded strata — high
    /// frequency across the layering, low along it, warped so the bands bend
    /// rather than run straight. Modulates the crystallized crust's hardness.
    fn foliation(&self, p: Vec3) -> f32 {
        let warp = fbm(p * (self.foliation_freq * 0.5), 2, 0xF0D1_A7E5, self.seed) as f32 - 0.5;
        let q = Vec3::new(
            p.x * self.foliation_freq + warp * 3.0,
            0.0,
            p.z * self.foliation_freq * 0.18,
        );
        let n = fbm(q, self.octaves, 0x57BA_7A00, self.seed);
        ((1.0 - 2.0 * (n - 0.5).abs()) * 2.0 - 1.0) as f32
    }

    /// Sample the cell at continuous world position `world` (cluster units) for a
    /// hex in state `state`, using the hex's own tectonic `elevation` / `orogeny`.
    pub fn sample(&self, state: &HexState, world: Vec2) -> CellSample {
        self.sample_blended(state, world, state.elevation, state.orogeny)
    }

    /// Like [`sample`](Self::sample), but with the tectonic `elevation` (`-1..1`)
    /// and `orogeny` (`0..1`) supplied by the caller instead of read from `state`.
    /// A renderer blends those two scalars across the hex's six-neighbour graph
    /// and feeds the smoothed values in, so terrain **joins across hex
    /// boundaries** rather than stepping at them. Composition, crust state and the
    /// ore vein still come from `state` (they vary by world position, not by the
    /// per-hex tectonic base).
    pub fn sample_blended(
        &self,
        state: &HexState,
        world: Vec2,
        elevation: f32,
        orogeny: f32,
    ) -> CellSample {
        self.sample_blended_at(state, Vec3::new(world.x, 0.0, world.y), elevation, orogeny)
    }

    /// Like [`sample_blended`](Self::sample_blended) but samples at an explicit 3D
    /// position (cluster units) rather than a flat `Vec2`. Used to sample over the
    /// sphere — the planet viewer feeds `dir * scale`, and because the fields are a
    /// function of the 3D point, neighbouring cells that share an edge position
    /// agree, so the within-hex terrain joins seam-free.
    pub fn sample_blended_at(
        &self,
        state: &HexState,
        pos: Vec3,
        elevation: f32,
        orogeny: f32,
    ) -> CellSample {
        let p = self.convect(pos);
        let surf = state.surface();
        let cutoff = surf.total() * self.trace_cutoff;

        // The hex's ore metal, if any, concentrates along a filament rather than
        // its smooth sub-field — a banded streak (strength tapers with the vein).
        let vein_metal = state.vein_element;
        let fil = if vein_metal.is_some() && state.vein_strength > 0.0 {
            self.filament(p) * state.vein_strength as f64
        } else {
            0.0
        };

        // Per-element NS sub-fields → per-cell composition → blended hardness.
        let (mut h_sum, mut w_sum) = (0.0f64, 0.0f64);
        let mut best: (Option<ElementId>, f64) = (None, f64::MIN);
        for (el, amount) in surf.iter() {
            let is_vein = Some(el) == vein_metal;
            // Trace elements don't move the blend — but never skip the vein metal,
            // whose whole point is to concentrate locally.
            if amount < cutoff && !is_vein {
                continue;
            }
            // The vein metal rides the filament (cleared from troughs, piled on
            // crests); every other element rides its smooth sub-field.
            let field = if is_vein {
                self.vein_low + (self.vein_gain - self.vein_low) * fil
            } else {
                let n = fbm(p * self.composition_freq, self.octaves, el as u64 ^ 0x00C0_FFEE, self.seed);
                self.floor + (1.0 - self.floor) * n
            };
            let local = amount * field;
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
        let mut hardness = if w_sum > 0.0 { (h_sum / w_sum) as f32 } else { 0.0 };

        // Foliation: once the crust has crystallized, fine strata band the
        // hardness up and down across the layering — the seam structure erosion
        // bites into at different rates.
        if !state.crust.is_empty() && self.foliation_amp != 0.0 {
            hardness = (hardness * (1.0 + self.foliation_amp * self.foliation(p))).clamp(0.0, 10.0);
        }

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
        if orogeny > 0.0 {
            let f = fbm(p * self.relief_freq * self.fold_freq_mult, self.octaves, 0x000F_01D5, self.seed);
            let fold = ((1.0 - 2.0 * (f - 0.5).abs()) * 2.0 - 1.0) as f32;
            ridge += orogeny * 0.6 * fold;
            amp *= 1.0 + orogeny * self.orogeny_lift;
        }
        let hard_norm = (hardness / 10.0).clamp(0.0, 1.0);
        let elev_world = elevation * self.tectonic_scale + amp * ridge * (0.3 + 0.7 * hard_norm);

        CellSample { hardness, elevation: elev_world, dominant: best.0, vein: fil as f32 }
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
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../Alpha/content/data");
        Tables::from_source(&JsonTableSource::new(dir)).expect("repo Alpha/content/data loads")
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
    fn sample_at_matches_the_flat_path() {
        let t = tables();
        let s = FieldSampler::new(&t, 7);
        let state = HexState::new(Composition::from_iter([(SI, 6000.0), (FE, 4000.0)]));
        let a = s.sample(&state, Vec2::new(1234.0, -567.0));
        let b = s.sample_blended_at(&state, Vec3::new(1234.0, 0.0, -567.0), state.elevation, state.orogeny);
        assert_eq!(a.hardness.to_bits(), b.hardness.to_bits());
        assert_eq!(a.elevation.to_bits(), b.elevation.to_bits());
        assert_eq!(a.vein.to_bits(), b.vein.to_bits());
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
        assert_eq!(a.vein.to_bits(), b.vein.to_bits());
    }

    #[test]
    fn an_ore_vein_bands_across_the_hex() {
        // A silica crust carrying an iron vein: the iron should concentrate into
        // filaments (some cells high-vein, most low) rather than spread evenly,
        // and the high-vein cells should read iron-ward in hardness (Fe 4.0 <
        // Si 6.5), giving the differential erosion handle.
        let t = tables();
        let s = FieldSampler::new(&t, 7);
        let mut state = HexState::new(Composition::new());
        state.crust = Composition::from_iter([(SI, 8000.0), (FE, 2000.0)]);
        state.vein_element = Some(FE);
        state.vein_strength = 1.0;

        let mut max_vein = 0.0f32;
        let mut min_vein = 1.0f32;
        let (mut soft_on_vein, mut hard_off_vein) = (false, false);
        for k in 0..400 {
            let w = Vec2::new(k as f32 * 53.0, (k as f32 * 37.0).sin() * 2000.0);
            let c = s.sample(&state, w);
            assert!((0.0..=1.0).contains(&c.vein));
            max_vein = max_vein.max(c.vein);
            min_vein = min_vein.min(c.vein);
            if c.vein > 0.5 && c.hardness < 6.0 {
                soft_on_vein = true; // iron pulled the blend down on the streak
            }
            if c.vein < 0.05 && c.hardness > 6.0 {
                hard_off_vein = true; // host silica between the bands
            }
        }
        assert!(max_vein - min_vein > 0.3, "vein didn't band (flat {min_vein}..{max_vein})");
        assert!(soft_on_vein, "vein cells didn't take on the metal's hardness");
        assert!(hard_off_vein, "host rock between the bands didn't stay hard");
    }

    #[test]
    fn no_vein_means_no_vein_signal() {
        let t = tables();
        let s = FieldSampler::new(&t, 7);
        let mut state = HexState::new(Composition::new());
        state.crust = Composition::from_iter([(SI, 9000.0), (FE, 1000.0)]);
        // vein_element stays None.
        for k in 0..100 {
            let c = s.sample(&state, Vec2::new(k as f32 * 91.0, k as f32 * 67.0));
            assert_eq!(c.vein, 0.0, "a vein signal appeared without a vein");
        }
    }

    #[test]
    fn foliation_bands_a_single_element_crust() {
        // A pure-silicon crust blends to a flat 6.5 hardness everywhere — so any
        // spatial variation here is the foliation alone. With it on, the hardness
        // bands; with it off, it's dead flat.
        let t = tables();
        let comp = Composition::from_iter([(SI, 10_000.0)]);
        let mut state = HexState::new(comp.clone());
        state.crust = comp; // crystallized → foliation applies

        let mut banded = FieldSampler::new(&t, 7);
        banded.foliation_amp = 0.3;
        let mut flat = FieldSampler::new(&t, 7);
        flat.foliation_amp = 0.0;

        let (mut blo, mut bhi) = (f32::MAX, f32::MIN);
        let (mut flo, mut fhi) = (f32::MAX, f32::MIN);
        for k in 0..200 {
            let w = Vec2::new(k as f32 * 60.0, k as f32 * 80.0);
            let b = banded.sample(&state, w).hardness;
            let f = flat.sample(&state, w).hardness;
            blo = blo.min(b);
            bhi = bhi.max(b);
            flo = flo.min(f);
            fhi = fhi.max(f);
        }
        assert!((fhi - flo) < 1e-3, "no-foliation hardness should be flat ({flo}..{fhi})");
        assert!((bhi - blo) > 0.5, "foliation didn't band the hardness ({blo}..{bhi})");
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

    #[test]
    fn blended_elevation_overrides_the_hex_base() {
        // The caller's blended elevation (not state.elevation) drives the cell —
        // the seam the renderer feeds in to join terrain across hex boundaries.
        let t = tables();
        let s = FieldSampler::new(&t, 7);
        let state = HexState::new(Composition::from_iter([(SI, 1000.0)])); // elevation 0
        let at = Vec2::new(200.0, 200.0);
        let low = s.sample_blended(&state, at, -0.5, 0.0);
        let high = s.sample_blended(&state, at, 0.5, 0.0);
        assert!(high.elevation > low.elevation, "blended elevation didn't drive the cell");
    }
}
