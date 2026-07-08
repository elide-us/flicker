//! The **condensation-class** composition — what a body is made of, grouped the
//! way planet formation actually turns on (what condenses where in the disk).
//!
//! # Why this exists alongside the element ledger
//!
//! A [`Body`](super::body::Body) carries *two* compositions, kept in sync (the
//! "store both" decision):
//!
//! - the conserved **element** vector ([`flicker_worldstate::Composition`]) — the
//!   ledger / Epoch-1 currency, keyed by atomic number; and
//! - this **class** breakdown — the *physics* truth that density, radius, gravity
//!   and pressure derive from.
//!
//! The class breakdown is load-bearing because the periodic table stores
//! **elemental** densities: oxygen is a gas (0.0014 g/cm³), yet a planet's oxygen
//! is bound in silicate rock (~3 g/cm³). A bulk density derived from the raw
//! element vector would treat a rock's oxygen as free gas and produce a wildly
//! wrong body. The five classes each carry a representative **material** density
//! ([`CondensationClass::density`]) that gets this right; the element vector is a
//! projection of the classes via their [`element makeup`](CondensationClass::element_makeup).
//!
//! The class→element direction is exact ([`ClassComposition::to_element_composition`]);
//! the reverse is ambiguous (oxygen lives in silicate, ice *and* carbon), which is
//! exactly why the class structure is retained rather than recovered.

use flicker_materials::ElementId;
use flicker_worldstate::Composition;

/// The five condensation classes, ordered **core → envelope by bulk density** —
/// the sequence a differentiated body layers them in, and the order erosive
/// collisions peel them off (gas first; see [`ClassComposition::strip_outermost`]).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum CondensationClass {
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

impl CondensationClass {
    /// Core → envelope (densest first). Index into [`ClassComposition::mass`] follows this.
    pub const ALL: [CondensationClass; 5] = [
        Self::Metal,
        Self::Silicate,
        Self::Carbon,
        Self::Ice,
        Self::Gas,
    ];

    /// Position in [`Self::ALL`] / [`ClassComposition::mass`].
    pub fn index(self) -> usize {
        Self::ALL.iter().position(|&c| c == self).unwrap()
    }

    /// Representative bulk **material** density (g/cm³) — how much volume a unit of
    /// this class's mass occupies. These set a differentiated body's bulk density
    /// and radius. They are *material* densities (rock, ice, metal …), deliberately
    /// not element densities (see the module docs).
    pub fn density(self) -> f64 {
        match self {
            Self::Metal => 7.9,
            Self::Silicate => 3.3,
            Self::Carbon => 2.2,
            Self::Ice => 1.0,
            Self::Gas => 0.15,
        }
    }

    /// Short label for a HUD legend.
    pub fn label(self) -> &'static str {
        match self {
            Self::Metal => "metal",
            Self::Silicate => "rock",
            Self::Carbon => "carbon",
            Self::Ice => "ice",
            Self::Gas => "gas",
        }
    }

    /// Display tint (RGB), warm/metallic → icy/pale — for colouring a body by its
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

    /// The class's Prism-element makeup as **relative mass weights** keyed by atomic
    /// number ([`ElementId`]); normalised on use, so they need not sum to 1. Every
    /// id here is a Prism element (asserted in the tests). This is the exact
    /// class→element map; deposit a class and these elements enter the ledger
    /// vector in proportion.
    pub fn element_makeup(self) -> &'static [(ElementId, f64)] {
        match self {
            // Fe/Ni metal core, with the usual siderophile minors (Co/S/P).
            Self::Metal => &[(26, 0.85), (28, 0.10), (27, 0.02), (16, 0.02), (15, 0.01)],
            // Bulk-silicate rock — O/Mg/Si dominated.
            Self::Silicate => &[
                (8, 0.44),
                (12, 0.21),
                (14, 0.21),
                (26, 0.06),
                (20, 0.025),
                (13, 0.024),
                (11, 0.006),
                (19, 0.004),
                (22, 0.002),
                (24, 0.003),
            ],
            // Refractory carbon / organics (CHON, carbon-rich).
            Self::Carbon => &[(6, 0.85), (8, 0.07), (1, 0.05), (7, 0.03)],
            // Water ice (H₂O dominant) plus volatile ices.
            Self::Ice => &[(8, 0.70), (1, 0.11), (6, 0.10), (7, 0.06), (16, 0.02), (17, 0.01)],
            // Nebular gas — solar H/He.
            Self::Gas => &[(1, 0.74), (2, 0.26)],
        }
    }
}

/// A body's composition as mass (M☉) per [`CondensationClass`], indexed by
/// [`CondensationClass::index`]. The physics view; the conserved element vector is
/// derived from it via [`Self::to_element_composition`].
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ClassComposition {
    /// Mass (M☉) per class, in [`CondensationClass::ALL`] order.
    pub mass: [f64; 5],
}

impl ClassComposition {
    /// An empty composition.
    pub fn new() -> Self {
        Self::default()
    }

    /// A composition holding `mass` (M☉) of a single class.
    pub fn of(class: CondensationClass, mass: f64) -> Self {
        let mut c = Self::new();
        c.mass[class.index()] = mass.max(0.0);
        c
    }

    /// Mass (M☉) of a single class.
    pub fn get(&self, class: CondensationClass) -> f64 {
        self.mass[class.index()]
    }

    /// Total mass across all classes (M☉).
    pub fn total(&self) -> f64 {
        self.mass.iter().sum()
    }

    /// True if the composition holds no mass.
    pub fn is_empty(&self) -> bool {
        self.total() <= 0.0
    }

    /// Add `mass` (M☉) of one class (negative is clamped to zero contribution).
    pub fn add_class(&mut self, class: CondensationClass, mass: f64) {
        self.mass[class.index()] += mass.max(0.0);
    }

    /// Fold `other` into `self` (mass-conserving union).
    pub fn add(&mut self, other: &ClassComposition) {
        for i in 0..5 {
            self.mass[i] += other.mass[i];
        }
    }

    /// Scale every class's mass by `f`.
    pub fn scaled(&self, f: f64) -> ClassComposition {
        let mut c = self.clone();
        for m in &mut c.mass {
            *m *= f;
        }
        c
    }

    /// Bulk volume (cm³): each class's mass / its material density, summed — a
    /// differentiated interior, not one mean density. Requires the mass unit to be
    /// solar masses (the model convention); pass it through [`M_SUN_G`].
    ///
    /// [`M_SUN_G`]: crate::units::M_SUN_G
    pub fn volume_cm3(&self) -> f64 {
        CondensationClass::ALL
            .iter()
            .map(|&c| self.get(c) * crate::units::M_SUN_G / c.density())
            .sum()
    }

    /// Bulk density (g/cm³) of the differentiated body; `0.0` for an empty body.
    pub fn density(&self) -> f64 {
        let v = self.volume_cm3();
        if v <= 0.0 {
            0.0
        } else {
            self.total() * crate::units::M_SUN_G / v
        }
    }

    /// The class holding the most mass (the body's "type"); `None` if empty.
    pub fn dominant(&self) -> Option<CondensationClass> {
        CondensationClass::ALL
            .iter()
            .copied()
            .filter(|&c| self.get(c) > 0.0)
            .max_by(|&a, &b| self.get(a).total_cmp(&self.get(b)))
    }

    /// Fraction of mass held as ice/water — a body's water proxy.
    pub fn water_fraction(&self) -> f64 {
        let t = self.total();
        if t <= 0.0 {
            0.0
        } else {
            self.get(CondensationClass::Ice) / t
        }
    }

    /// Remove `amount` (M☉) from the **outermost** classes first (gas → ice →
    /// carbon → silicate → metal), returning what was removed. Models a collision
    /// peeling off envelope and mantle before reaching the dense core — the
    /// mechanism behind iron-rich (Mercury-like) and desiccated remnants. Caps at
    /// the available mass.
    pub fn strip_outermost(&mut self, amount: f64) -> ClassComposition {
        let mut removed = ClassComposition::new();
        let mut remaining = amount.max(0.0);
        for &class in CondensationClass::ALL.iter().rev() {
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

    /// Remove a `fraction` (0..=1) of **every** class, returning what was removed —
    /// a representative draw of the mix (a body sweeping up part of a cloud ring,
    /// say). Conservation-safe: the removed plus the remainder equal the original.
    /// `fraction` is clamped to `0..=1`.
    pub fn take_fraction(&mut self, fraction: f64) -> ClassComposition {
        let f = fraction.clamp(0.0, 1.0);
        let mut removed = ClassComposition::new();
        for i in 0..5 {
            let take = self.mass[i] * f;
            self.mass[i] -= take;
            removed.mass[i] = take;
        }
        removed
    }

    /// Remove up to `amount` (M☉) of total mass, taken **proportionally** across the
    /// classes present, returning what was removed — never more than is held. The
    /// accretion-accounting primitive: a body draws a representative slice of a
    /// reservoir's material. Caps at the available mass.
    pub fn take_mass(&mut self, amount: f64) -> ClassComposition {
        let total = self.total();
        if total <= 0.0 || amount <= 0.0 {
            return ClassComposition::new();
        }
        self.take_fraction(amount / total)
    }

    /// Project to the conserved **element** vector ([`flicker_worldstate::Composition`],
    /// keyed by atomic number): each class contributes its
    /// [`element makeup`](CondensationClass::element_makeup) scaled so the elements
    /// sum to the class's mass. Total element mass equals [`Self::total`]. This is
    /// the exact class→element bridge; it is also the Epoch-1 abundance source (a
    /// normalise away).
    pub fn to_element_composition(&self) -> Composition {
        let mut out = Composition::new();
        for &class in &CondensationClass::ALL {
            let cm = self.get(class);
            if cm <= 0.0 {
                continue;
            }
            let makeup = class.element_makeup();
            let wsum: f64 = makeup.iter().map(|(_, w)| *w).sum();
            if wsum <= 0.0 {
                continue;
            }
            for &(element, w) in makeup {
                out.add(element, cm * w / wsum);
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every atomic number a class can emit is a Prism element (mirrors the
    /// solarsystem `every_emitted_symbol_is_in_prism` guard, by number).
    #[test]
    fn every_makeup_element_is_in_prism() {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../Alpha/content/data");
        let source = flicker_materials::JsonTableSource::new(dir);
        let tables = flicker_materials::Tables::from_source(&source).expect("tables load");
        for &class in &CondensationClass::ALL {
            for &(element, w) in class.element_makeup() {
                assert!(w > 0.0, "{class:?} weight must be positive");
                assert!(
                    tables.element_by_number(element).is_some(),
                    "atomic number {element} (in {class:?}) is not a Prism element",
                );
            }
        }
    }

    #[test]
    fn element_projection_conserves_class_mass() {
        // A single class projects to elements whose masses sum to the class mass.
        let c = ClassComposition::of(CondensationClass::Silicate, 2.0);
        let total: f64 = c.to_element_composition().total();
        assert!((total - 2.0).abs() < 1e-9, "element masses conserve class mass, got {total}");
    }

    #[test]
    fn merge_conserves_mass() {
        let mut a = ClassComposition::of(CondensationClass::Metal, 1.0);
        let b = {
            let mut b = ClassComposition::of(CondensationClass::Ice, 0.5);
            b.add_class(CondensationClass::Silicate, 0.25);
            b
        };
        let before = a.total() + b.total();
        a.add(&b);
        assert!((a.total() - before).abs() < 1e-12);
        assert_eq!(a.get(CondensationClass::Metal), 1.0);
        assert_eq!(a.get(CondensationClass::Ice), 0.5);
    }

    #[test]
    fn strip_removes_outermost_first() {
        let mut c = ClassComposition::new();
        c.add_class(CondensationClass::Metal, 1.0);
        c.add_class(CondensationClass::Silicate, 1.0);
        c.add_class(CondensationClass::Ice, 1.0);
        c.add_class(CondensationClass::Gas, 0.5);
        let removed = c.strip_outermost(0.7);
        assert!((removed.total() - 0.7).abs() < 1e-12, "removed the requested mass");
        assert_eq!(c.get(CondensationClass::Gas), 0.0, "all gas peeled first");
        assert!((c.get(CondensationClass::Ice) - 0.8).abs() < 1e-12, "then 0.2 of the ice");
        assert_eq!(c.get(CondensationClass::Metal), 1.0, "core untouched");
    }

    #[test]
    fn iron_is_dense_ice_is_not() {
        let iron = ClassComposition::of(CondensationClass::Metal, 1.0);
        let icy = ClassComposition::of(CondensationClass::Ice, 1.0);
        assert!(iron.density() > 7.0 && iron.density() < 8.0);
        assert!(icy.density() < 1.5);
        assert!(iron.density() > icy.density());
    }
}
