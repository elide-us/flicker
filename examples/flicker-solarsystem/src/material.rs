//! Composition model — what a forming body is *made of*, tracked strictly within
//! Prism's limited periodic table (`data/materials/periodic_table.json`, loaded
//! via [`flicker_materials::Tables`]).
//!
//! A body's composition is mass split across five **condensation classes** — the
//! coarse grouping planet formation actually turns on (what condenses where in the
//! disk): refractory **metal** (Fe/Ni), **silicate** rock, refractory **carbon**,
//! **ice** (water + volatile ices, only past the snow line), and nebular **gas**
//! (an H/He envelope). Each class resolves to Prism elements via [`CLASS_ELEMENTS`];
//! every symbol there is checked against the loaded table (see
//! `every_emitted_symbol_is_in_prism`), so the sim can never invent an element the
//! game doesn't have.
//!
//! We deliberately do **no** `materials[256]` mineralogy classification — that
//! resolved-material layer is Epoch 2's job (a flagged TBD in the material-model
//! handoff). Formation stops at an **element-abundance vector** ([`Composition::to_epoch1_abundance`]),
//! which is exactly the currency `flicker_worldgen`'s Epoch 1 consumes.

use std::collections::HashMap;

use flicker_materials::{JsonTableSource, Tables};

use crate::astro::M_SUN_G;

/// The five condensation classes, ordered **core → envelope by bulk density**.
/// That order is the sequence a differentiated body layers them in, and the order
/// erosive collisions peel them off (outermost — gas — first; see
/// [`Composition::strip_outermost`]). Stripping the mantle/volatiles before the
/// dense metal core is the physical mechanism behind iron-rich (Mercury-like) and
/// desiccated remnants — diversity for free.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum MaterialClass {
    /// Fe/Ni metal — the core.
    Metal,
    /// Silicate rock — the mantle.
    Silicate,
    /// Refractory carbon / organics.
    Carbon,
    /// Water ice + volatile ices (only condenses past the snow line).
    Ice,
    /// Nebular H/He — an envelope, only retained by massive cores.
    Gas,
}

impl MaterialClass {
    /// Core → envelope (densest first). Index into [`Composition::mass`] follows this.
    pub const ALL: [MaterialClass; 5] = [
        Self::Metal,
        Self::Silicate,
        Self::Carbon,
        Self::Ice,
        Self::Gas,
    ];

    /// Position in [`Self::ALL`] / [`Composition::mass`].
    pub fn index(self) -> usize {
        Self::ALL.iter().position(|&c| c == self).unwrap()
    }

    /// Bulk material density (g/cm³) — how much volume a unit of this class's mass
    /// occupies, which sets a differentiated body's bulk density and radius. These
    /// are representative *material* densities (rock, ice, …), not element densities.
    pub fn density(self) -> f64 {
        match self {
            Self::Metal => 7.9,
            Self::Silicate => 3.3,
            Self::Carbon => 2.2,
            Self::Ice => 1.0,
            Self::Gas => 0.15,
        }
    }

    /// Short label for the HUD legend.
    pub fn label(self) -> &'static str {
        match self {
            Self::Metal => "metal",
            Self::Silicate => "rock",
            Self::Carbon => "carbon",
            Self::Ice => "ice",
            Self::Gas => "gas",
        }
    }

    /// Display tint (RGB), warm/metallic → icy/pale, used to colour bodies by their
    /// dominant class.
    pub fn color(self) -> [f32; 3] {
        match self {
            Self::Metal => [0.74, 0.70, 0.62],
            Self::Silicate => [0.78, 0.52, 0.34],
            Self::Carbon => [0.32, 0.30, 0.30],
            Self::Ice => [0.62, 0.86, 0.96],
            Self::Gas => [0.74, 0.78, 0.92],
        }
    }
}

/// Each class → its Prism-element makeup as **relative mass weights** (normalised
/// on use, so they need not sum to 1). Only symbols present in
/// `data/materials/periodic_table.json` appear — `every_emitted_symbol_is_in_prism`
/// enforces it. Authored to land near bulk-silicate / core / volatile proportions
/// for an Earth-like body; **Mg is a major silicate component** now that it's in
/// the table.
pub const CLASS_ELEMENTS: [(MaterialClass, &[(&str, f64)]); 5] = [
    // Fe/Ni metal core, with the usual siderophile minors.
    (
        MaterialClass::Metal,
        &[("Fe", 0.85), ("Ni", 0.10), ("Co", 0.02), ("S", 0.02), ("P", 0.01)],
    ),
    // Bulk-silicate rock — O/Mg/Si dominated, Mg-bearing (Prism now has Mg).
    (
        MaterialClass::Silicate,
        &[
            ("O", 0.44),
            ("Mg", 0.21),
            ("Si", 0.21),
            ("Fe", 0.06),
            ("Ca", 0.025),
            ("Al", 0.024),
            ("Na", 0.006),
            ("K", 0.004),
            ("Ti", 0.002),
            ("Cr", 0.003),
        ],
    ),
    // Refractory carbon / organics (CHON, carbon-rich).
    (
        MaterialClass::Carbon,
        &[("C", 0.85), ("O", 0.07), ("H", 0.05), ("N", 0.03)],
    ),
    // Water ice (H₂O dominant) plus volatile ices (CO/CO₂, ammonia, salts).
    (
        MaterialClass::Ice,
        &[("O", 0.70), ("H", 0.11), ("C", 0.10), ("N", 0.06), ("S", 0.02), ("Cl", 0.01)],
    ),
    // Nebular gas — solar H/He.
    (MaterialClass::Gas, &[("H", 0.74), ("He", 0.26)]),
];

/// A body's composition: mass (in **solar masses**, matching [`crate::body::Body::mass`])
/// per [`MaterialClass`], indexed by [`MaterialClass::index`].
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Composition {
    pub mass: [f64; 5],
}

impl Composition {
    /// An empty composition.
    pub fn new() -> Self {
        Self::default()
    }

    /// A composition holding `mass` (M☉) of a single class.
    pub fn of(class: MaterialClass, mass: f64) -> Self {
        let mut c = Self::new();
        c.mass[class.index()] = mass.max(0.0);
        c
    }

    /// Mass (M☉) of a single class.
    pub fn get(&self, class: MaterialClass) -> f64 {
        self.mass[class.index()]
    }

    /// Total mass across all classes (M☉).
    pub fn total(&self) -> f64 {
        self.mass.iter().sum()
    }

    /// Fold `other` into `self` (mass-conserving union).
    pub fn add(&mut self, other: &Composition) {
        for i in 0..5 {
            self.mass[i] += other.mass[i];
        }
    }

    /// Scale every class's mass by `f` (e.g. taking a fraction of a body).
    pub fn scaled(&self, f: f64) -> Composition {
        let mut c = self.clone();
        for m in &mut c.mass {
            *m *= f;
        }
        c
    }

    /// Bulk volume (cm³) summing each class's mass / its material density —
    /// i.e. a differentiated interior, not a single mean density.
    pub fn volume_cm3(&self) -> f64 {
        MaterialClass::ALL
            .iter()
            .map(|&c| self.get(c) * M_SUN_G / c.density())
            .sum()
    }

    /// Bulk density (g/cm³) of the differentiated body; `0` for an empty composition.
    pub fn density(&self) -> f64 {
        let v = self.volume_cm3();
        if v <= 0.0 {
            0.0
        } else {
            self.total() * M_SUN_G / v
        }
    }

    /// The class holding the most mass (the body's "type"); `None` if empty.
    pub fn dominant(&self) -> Option<MaterialClass> {
        MaterialClass::ALL
            .iter()
            .copied()
            .filter(|&c| self.get(c) > 0.0)
            .max_by(|&a, &b| self.get(a).total_cmp(&self.get(b)))
    }

    /// Fraction of the body's mass held as ice/water — the verdict's water proxy.
    pub fn water_fraction(&self) -> f64 {
        let t = self.total();
        if t <= 0.0 {
            0.0
        } else {
            self.get(MaterialClass::Ice) / t
        }
    }

    /// Remove `amount` (M☉) from the **outermost** classes first (gas → ice →
    /// carbon → silicate → metal), returning what was removed. Models a collision
    /// peeling off envelope and mantle before it ever reaches the dense core. Caps
    /// at the available mass.
    pub fn strip_outermost(&mut self, amount: f64) -> Composition {
        let mut removed = Composition::new();
        let mut remaining = amount.max(0.0);
        for &class in MaterialClass::ALL.iter().rev() {
            if remaining <= 0.0 {
                break;
            }
            let i = class.index();
            let take = self.mass[i].min(remaining);
            self.mass[i] -= take;
            removed.mass[i] += take;
            remaining -= take;
        }
        removed
    }

    /// Absolute element masses (M☉, summed across classes), keyed by Prism symbol.
    pub fn to_element_masses(&self) -> HashMap<String, f64> {
        let mut out: HashMap<String, f64> = HashMap::new();
        for (class, elems) in CLASS_ELEMENTS {
            let cm = self.get(class);
            if cm <= 0.0 {
                continue;
            }
            let wsum: f64 = elems.iter().map(|(_, w)| *w).sum();
            for (sym, w) in elems {
                *out.entry((*sym).to_string()).or_insert(0.0) += cm * w / wsum;
            }
        }
        out
    }

    /// The body's bulk composition as a **relative element abundance** (mass-%),
    /// keyed by Prism symbol — the vector `Epoch1Params.abundance` consumes. Only
    /// symbols present in `tables` survive (the in-bounds guard; all should pass by
    /// construction). Empty if the body has no mass in known elements.
    pub fn to_epoch1_abundance(&self, tables: &Tables) -> HashMap<String, f64> {
        let masses = self.to_element_masses();
        let total: f64 = masses
            .iter()
            .filter(|(s, _)| tables.element(s).is_some())
            .map(|(_, m)| *m)
            .sum();
        if total <= 0.0 {
            return HashMap::new();
        }
        masses
            .into_iter()
            .filter(|(s, _)| tables.element(s).is_some())
            .map(|(s, m)| (s, m / total * 100.0))
            .collect()
    }
}

/// The repo material data directory (the canonical Prism source), relative to this
/// example's manifest — the same files world-gen loads.
const MATERIALS_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../data/materials");

/// Load the Prism vocabulary (periodic table + materials) from `data/materials`.
/// The authoritative element set the sim's chemistry must stay within.
pub fn load_tables() -> Tables {
    Tables::from_source(&JsonTableSource::new(MATERIALS_DIR)).expect("repo data/materials loads")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_emitted_symbol_is_in_prism() {
        let t = load_tables();
        for (class, elems) in CLASS_ELEMENTS {
            for (sym, w) in elems {
                assert!(*w > 0.0, "{} weight must be positive", sym);
                assert!(
                    t.element(sym).is_some(),
                    "{sym} (in {:?}) is not a Prism element",
                    class
                );
            }
        }
    }

    #[test]
    fn single_class_resolves_to_its_own_mass() {
        // A pure-silicate body's element masses sum to its mass (weights normalise).
        let c = Composition::of(MaterialClass::Silicate, 2.0);
        let total: f64 = c.to_element_masses().values().sum();
        assert!((total - 2.0).abs() < 1e-9, "element masses conserve class mass");
    }

    #[test]
    fn merge_conserves_mass() {
        let a = Composition::of(MaterialClass::Metal, 1.0);
        let mut b = Composition::of(MaterialClass::Ice, 0.5);
        b.add(&Composition::of(MaterialClass::Silicate, 0.25));
        let before = a.total() + b.total();
        let mut m = a.clone();
        m.add(&b);
        assert!((m.total() - before).abs() < 1e-12, "merge conserves total mass");
        assert_eq!(m.get(MaterialClass::Metal), 1.0);
        assert_eq!(m.get(MaterialClass::Ice), 0.5);
    }

    #[test]
    fn strip_removes_outermost_first() {
        // metal core 1.0, rock 1.0, ice 1.0, gas 0.5 → strip 0.7 takes gas then ice.
        let mut c = Composition::new();
        c.add(&Composition::of(MaterialClass::Metal, 1.0));
        c.add(&Composition::of(MaterialClass::Silicate, 1.0));
        c.add(&Composition::of(MaterialClass::Ice, 1.0));
        c.add(&Composition::of(MaterialClass::Gas, 0.5));
        let removed = c.strip_outermost(0.7);
        assert!((removed.total() - 0.7).abs() < 1e-12, "removed the requested mass");
        assert_eq!(c.get(MaterialClass::Gas), 0.0, "all gas peeled first");
        assert!((c.get(MaterialClass::Ice) - 0.8).abs() < 1e-12, "then 0.2 of the ice");
        assert_eq!(c.get(MaterialClass::Metal), 1.0, "core untouched");
    }

    #[test]
    fn iron_world_is_dense_water_world_is_not() {
        let iron = Composition::of(MaterialClass::Metal, 1.0);
        let icy = Composition::of(MaterialClass::Ice, 1.0);
        assert!(iron.density() > 7.0 && iron.density() < 8.0);
        assert!(icy.density() < 1.5);
        assert!(iron.density() > icy.density());
    }
}
