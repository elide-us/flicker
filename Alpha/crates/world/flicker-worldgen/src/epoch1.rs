//! **Epoch 1 — initial element distribution** (world-gen spec, Epoch 1).
//!
//! Establishes bulk composition per hex from initial planetary conditions:
//! distribute element mass across the hex array with smooth large-scale
//! variation, **biasing heavy (dense) elements toward the equator** (a proxy for
//! the accretion plane) and **volatile (gas) elements toward the poles**, with
//! correlated noise for regional provinces. Operates at the hex level — one
//! composition vector per hex — and is the first runnable epoch kernel.
//!
//! The kernel is **pure and seeded**: it takes a unit-sphere direction per hex
//! (the hex's point on the planet, e.g. `HexMap::celestial_dir`) so it stays
//! decoupled from the topology crate, and the same `(seed, dir)` always yields
//! the same composition.

use std::collections::HashMap;

use flicker_materials::{PhysicalState, Tables};
use flicker_worldstate::Composition;
use glam::Vec3;
use serde::{Deserialize, Serialize};

use crate::noise::fbm;

/// Epoch 1 generation parameters — the planet's *birth certificate* (system
/// spec §3.2), not periodic-table data. Serializable so a world's lineage can be
/// recorded. [`Default`] approximates Earth's crust; every field is meant to be
/// tuned.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Epoch1Params {
    /// Initial element abundance ratios, keyed by chemical symbol. The Epoch 1
    /// "abundance ratios" input — **not** in `periodic_table.json` (abundance is
    /// a generation parameter, not an element property). Relative, not absolute:
    /// each hex is renormalized to [`target_mass`](Self::target_mass).
    pub abundance: HashMap<String, f64>,
    /// Abundance weight for any element absent from [`abundance`](Self::abundance)
    /// — the trace floor (rare metals, minor gases).
    pub abundance_floor: f64,
    /// Total element mass each hex's composition is normalized to.
    pub target_mass: f64,
    /// Equatorial enrichment of heavy elements: an element of full heaviness gets
    /// up to `×(1 + equator_bias)` at the equator. `0` disables.
    pub equator_bias: f64,
    /// Polar enrichment of volatile (gas-state) elements: up to `×(1 + pole_bias)`
    /// at the poles. `0` disables.
    pub pole_bias: f64,
    /// Density (g/cm³) that maps to full "heaviness" 1.0; denser elements clamp
    /// there.
    pub density_ref: f64,
    /// Spatial frequency of the regional noise over the unit sphere — higher is
    /// finer-grained provinces.
    pub frequency: f32,
    /// Octaves of regional noise.
    pub octaves: u32,
    /// Contrast applied to the regional noise about its midpoint
    /// (`(n − 0.5)·contrast + 0.5`, clamped): `1.0` leaves fBm as-is (which
    /// concentrates near 0.5 — nearly uniform compositions); `> 1` spreads it
    /// toward the extremes so the competitive rock-formers actually trade
    /// dominance and provinces emerge.
    pub contrast: f64,
    /// Fraction of an element's weight kept where its noise field is `0`, so a
    /// composition never fully vanishes in a trough. Lower → higher contrast
    /// (more elements can locally dominate).
    pub noise_floor: f64,
}

impl Default for Epoch1Params {
    fn default() -> Self {
        // Approx Earth crustal mass-% for the elements in the current table;
        // anything unlisted falls to `abundance_floor`. Birth-certificate values
        // — adjust to author a different planet.
        let abundance = [
            ("O", 46.0),
            ("Si", 28.0),
            ("Al", 8.0),
            ("Fe", 5.0),
            ("Ca", 3.6),
            ("Na", 2.8),
            ("K", 2.6),
            ("Ti", 0.6),
            ("C", 0.2),
            ("H", 0.14),
            ("P", 0.1),
            ("S", 0.05),
            ("Cl", 0.05),
            ("N", 0.02),
        ]
        .into_iter()
        .map(|(s, w)| (s.to_string(), w))
        .collect();
        Self {
            abundance,
            abundance_floor: 0.01,
            target_mass: 10_000.0,
            equator_bias: 1.5,
            pole_bias: 1.5,
            density_ref: 8.0,
            frequency: 2.2,
            octaves: 2,
            // Sharper provinces (was 3.0) so regions reach stronger single-element
            // dominance — including the heavy/iron-rich regions that give Epoch 2 a
            // wider crust-buoyancy spread for Epoch 3's isostasy to carve.
            contrast: 4.0,
            noise_floor: 0.05,
        }
    }
}

/// The Epoch 1 kernel: a vocabulary, the parameters, and the world seed.
pub struct Epoch1<'a> {
    tables: &'a Tables,
    params: Epoch1Params,
    seed: u64,
}

impl<'a> Epoch1<'a> {
    /// A kernel reading element traits from `tables`, parameterized by `params`,
    /// seeded by `seed`.
    pub fn new(tables: &'a Tables, params: Epoch1Params, seed: u64) -> Self {
        Self {
            tables,
            params,
            seed,
        }
    }

    /// The generation parameters (the birth certificate).
    pub fn params(&self) -> &Epoch1Params {
        &self.params
    }

    /// Seed the composition of one hex at unit-sphere direction `dir`. `dir.y` is
    /// the sine of latitude (`0` at the equator, `±1` at the poles), driving the
    /// heavy/volatile bias; the 3D point drives the regional noise. The result is
    /// normalized to [`Epoch1Params::target_mass`].
    pub fn seed_hex(&self, dir: Vec3) -> Composition {
        let dir = dir.normalize_or_zero();
        let poleness = (dir.y.abs() as f64).clamp(0.0, 1.0);
        let equatorness = 1.0 - poleness;

        let p = &self.params;
        let mut comp = Composition::new();
        for e in self.tables.elements() {
            let base = p
                .abundance
                .get(&e.symbol)
                .copied()
                .unwrap_or(p.abundance_floor);
            if base <= 0.0 {
                continue;
            }
            // Heavy → equator; volatile (gas) → pole.
            let heaviness = (e.density_g_cm3 as f64 / p.density_ref).clamp(0.0, 1.0);
            let volatility = (e.state == PhysicalState::Gas) as i32 as f64;
            let bias = 1.0
                + p.equator_bias * heaviness * equatorness
                + p.pole_bias * volatility * poleness;
            // Correlated regional noise, an independent field per element,
            // sharpened about its midpoint so provinces emerge.
            let n = fbm(dir * p.frequency, p.octaves, e.number as u64, self.seed);
            let n = ((n - 0.5) * p.contrast + 0.5).clamp(0.0, 1.0);
            let regional = p.noise_floor + (1.0 - p.noise_floor) * n;

            let amount = base * bias * regional;
            if amount > 0.0 {
                comp.add(e.number, amount);
            }
        }

        // Normalize each hex to the same total mass so amounts are comparable and
        // "dominant" is well-defined across the array.
        let total = comp.total();
        if total > 0.0 {
            let scale = p.target_mass / total;
            comp = comp.iter().map(|(el, a)| (el, a * scale)).collect();
        }
        comp
    }

    /// Seed every hex from its unit-sphere direction (e.g.
    /// `(0..map.total()).map(|i| map.celestial_dir(i).into())`).
    pub fn seed_world(&self, dirs: impl IntoIterator<Item = Vec3>) -> Vec<Composition> {
        dirs.into_iter().map(|d| self.seed_hex(d)).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    use flicker_materials::{ElementId, JsonTableSource};

    fn tables() -> Tables {
        let dir = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../../Alpha/content/data"
        );
        Tables::from_source(&JsonTableSource::new(dir)).expect("repo Alpha/content/data loads")
    }

    /// A Fibonacci-sphere of `n` unit directions — an even spread of "hexes" to
    /// sample the distribution over, standing in for the real topology.
    fn sphere_dirs(n: usize) -> Vec<Vec3> {
        let golden = std::f32::consts::PI * (3.0 - 5.0f32.sqrt());
        (0..n)
            .map(|i| {
                let y = 1.0 - (i as f32 / (n as f32 - 1.0)) * 2.0; // 1 → -1
                let r = (1.0 - y * y).max(0.0).sqrt();
                let a = golden * i as f32;
                Vec3::new(a.cos() * r, y, a.sin() * r)
            })
            .collect()
    }

    fn dominant_symbol(t: &Tables, c: &Composition) -> String {
        c.dominant()
            .and_then(|id| t.element_by_number(id))
            .map(|e| e.symbol.clone())
            .unwrap_or_default()
    }

    #[test]
    fn every_hex_normalizes_to_target_mass() {
        let t = tables();
        let e1 = Epoch1::new(&t, Epoch1Params::default(), 1234);
        for d in sphere_dirs(200) {
            let c = e1.seed_hex(d);
            assert!(
                (c.total() - 10_000.0).abs() < 1e-3,
                "total {} off target",
                c.total()
            );
            assert!(!c.is_empty());
        }
    }

    #[test]
    fn deterministic_and_seed_sensitive() {
        let t = tables();
        let d = Vec3::new(0.2, 0.5, 0.84).normalize();
        let a = Epoch1::new(&t, Epoch1Params::default(), 42).seed_hex(d);
        let b = Epoch1::new(&t, Epoch1Params::default(), 42).seed_hex(d);
        assert_eq!(a, b, "same seed+dir must reproduce");
        let c = Epoch1::new(&t, Epoch1Params::default(), 43).seed_hex(d);
        assert_ne!(a, c, "different seed should differ");
    }

    #[test]
    fn varies_regionally() {
        // Distant hexes get materially different compositions (the noise works).
        let t = tables();
        let e1 = Epoch1::new(&t, Epoch1Params::default(), 7);
        let north = e1.seed_hex(Vec3::new(0.0, 1.0, 0.0));
        let here = e1.seed_hex(Vec3::new(1.0, 0.0, 0.0));
        let there = e1.seed_hex(Vec3::new(-1.0, 0.0, 0.0));
        assert_ne!(north, here);
        assert_ne!(here, there);
    }

    #[test]
    fn heavy_elements_enrich_toward_the_equator() {
        // Average Fe (dense) over an equatorial band vs a polar cap; the bias
        // must make the equator richer once noise averages out.
        let t = tables();
        let e1 = Epoch1::new(&t, Epoch1Params::default(), 99);
        let fe: ElementId = 26;
        let mean = |dirs: &[Vec3]| -> f64 {
            dirs.iter().map(|&d| e1.seed_hex(d).amount(fe)).sum::<f64>() / dirs.len() as f64
        };
        let n = 48;
        let equator: Vec<Vec3> = (0..n)
            .map(|i| {
                let a = i as f32 / n as f32 * std::f32::consts::TAU;
                Vec3::new(a.cos(), 0.0, a.sin())
            })
            .collect();
        let pole: Vec<Vec3> = (0..n)
            .map(|i| {
                let a = i as f32 / n as f32 * std::f32::consts::TAU;
                Vec3::new(a.cos() * 0.15, 0.99, a.sin() * 0.15)
            })
            .collect();
        assert!(mean(&equator) > mean(&pole), "Fe not equator-enriched");
    }

    #[test]
    fn volatile_elements_enrich_toward_the_poles() {
        // Nitrogen (gas) should band toward the poles.
        let t = tables();
        let e1 = Epoch1::new(&t, Epoch1Params::default(), 99);
        let n2: ElementId = 7;
        let mean = |dirs: &[Vec3]| -> f64 {
            dirs.iter().map(|&d| e1.seed_hex(d).amount(n2)).sum::<f64>() / dirs.len() as f64
        };
        let count = 48;
        let equator: Vec<Vec3> = (0..count)
            .map(|i| {
                let a = i as f32 / count as f32 * std::f32::consts::TAU;
                Vec3::new(a.cos(), 0.0, a.sin())
            })
            .collect();
        let pole: Vec<Vec3> = (0..count)
            .map(|i| {
                let a = i as f32 / count as f32 * std::f32::consts::TAU;
                Vec3::new(a.cos() * 0.15, 0.99, a.sin() * 0.15)
            })
            .collect();
        assert!(mean(&pole) > mean(&equator), "N not pole-enriched");
    }

    #[test]
    fn distribution_has_several_dominant_elements() {
        // The payoff: across the array the dominant element varies — provinces
        // emerge, not one flat composition. Also prints the histogram so a human
        // can "see and verify" with `--nocapture`.
        let t = tables();
        let e1 = Epoch1::new(&t, Epoch1Params::default(), 2024);
        let dirs = sphere_dirs(300);
        let mut hist: BTreeMap<String, usize> = BTreeMap::new();
        for d in &dirs {
            *hist
                .entry(dominant_symbol(&t, &e1.seed_hex(*d)))
                .or_default() += 1;
        }
        println!(
            "Epoch 1 dominant-element distribution over {} hexes:",
            dirs.len()
        );
        for (sym, n) in &hist {
            println!("  {sym:>3}: {n}");
        }
        assert!(
            hist.len() >= 3,
            "only {} distinct dominants: {:?}",
            hist.len(),
            hist
        );
    }
}
