//! [`Body`] — a physical object in a star system, carrying the four fields the
//! spec is explicit about: **composition**, **gravity**, **density** and
//! **pressure**.
//!
//! A body is a *node in a tree*: it owns its [`Satellite`]s, and a satellite may
//! itself be a [`Body`] (a moon, whose moons recurse). Kinematic state
//! (`pos`/`vel`) is stored **relative to the parent's frame** — heliocentric for a
//! planet, planetocentric for a moon — so the same orbital math serves every level
//! of the tree once given the parent's mass. The root star sits at rest at the
//! origin of its own frame.
//!
//! # Two compositions, both driven by what the body absorbs
//!
//! Following the "store both" decision (see [`condensation`](super::condensation)),
//! a body carries:
//!
//! - [`classes`](Body::classes) — the **condensation-class** breakdown: the
//!   *physics* truth that density, radius, gravity and pressure derive from (class
//!   bulk densities are correct where raw elemental densities are not).
//! - [`composition`](Body::composition) — the conserved **element** vector
//!   ([`flicker_worldstate::Composition`]): the ledger / Epoch-1 currency.
//!
//! These are not two independent copies kept identical — *one drives the other*. A
//! body **grows by absorbing classed material** ([`Body::absorb`] / [`Body::deposit`]),
//! and every absorption adds class mass and, with it, the element projection of that
//! mass ([`ClassComposition::to_element_composition`]). The cloud→body side of that
//! is a conservation transfer — material *removed from the cloud*
//! ([`Cloud`](super::cloud::Cloud)) is *inserted into the body* — so the two
//! representations can't disagree because both accumulate from the same transfers.
//! `pos`/`vel`/`kind` are mutated freely; the compositions only ever change through
//! [`Body::absorb`] / [`Body::deposit`] / [`Body::strip`].

use flicker_materials::ElementId;
use flicker_worldstate::Composition;
use glam::DVec3;

use crate::units::{AU_CM, AU_M, G, G_SI, M_SUN_KG};

use super::condensation::{ClassComposition, CondensationClass};
use super::satellite::Satellite;

/// What a body physically *is* — its formation outcome. Intrinsic and stored; the
/// IAU-style label (planet vs dwarf, moon, comet) is *contextual* and derived from
/// the tree instead (see [`Classification`](super::system::Classification)).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum BodyKind {
    /// A fusing star — the root of a [`System`](super::system::System).
    Star,
    /// A rocky/icy body: an embryo, a protoplanet, or their merger products (the
    /// default for anything that didn't capture a gas envelope).
    Protoplanet,
    /// A gas giant: a core past the snow line that captured an H/He envelope.
    Giant,
    /// A collision fragment (erosion / disruption debris).
    Debris,
}

impl BodyKind {
    /// Whether this is a gas giant (captured an envelope).
    pub fn is_giant(self) -> bool {
        matches!(self, BodyKind::Giant)
    }

    /// Whether this is the central star.
    pub fn is_star(self) -> bool {
        matches!(self, BodyKind::Star)
    }
}

/// One body in a star system — a recursive tree node (see the module docs).
#[derive(Clone, Debug)]
pub struct Body {
    /// Position in the **parent's frame** (AU). The root star is at the origin.
    pub pos: DVec3,
    /// Velocity in the **parent's frame** (AU/yr). The root star is at rest.
    pub vel: DVec3,
    /// What the body physically is (its formation outcome).
    pub kind: BodyKind,
    /// The conserved element-mass distribution (M☉ per element) — the ledger /
    /// Epoch-1 currency, kept in sync with [`Self::classes`]. Same [`Composition`]
    /// type the world ledger and Epoch 1 consume, so a selected body feeds straight
    /// into world-gen. Maintained by [`Self::deposit`] / [`Self::strip`].
    pub composition: Composition,
    /// The condensation-class breakdown — the physics truth density/radius/gravity/
    /// pressure derive from. Maintained by [`Self::deposit`] / [`Self::strip`].
    pub classes: ClassComposition,
    /// Bound satellites: child bodies (moons, submoons, comets) and discs of
    /// material (rings, belts). See [`Satellite`].
    pub satellites: Vec<Satellite>,
}

impl Body {
    /// An empty body at a given orbital state. Use [`Self::deposit`] (or
    /// [`Self::from_classes`]) to give it mass.
    pub fn new(pos: DVec3, vel: DVec3, kind: BodyKind) -> Self {
        Self {
            pos,
            vel,
            kind,
            composition: Composition::new(),
            classes: ClassComposition::new(),
            satellites: Vec::new(),
        }
    }

    /// A body built from a condensation-class composition; the element vector is
    /// derived to match. The constructor formation uses once a body's bulk is known.
    pub fn from_classes(pos: DVec3, vel: DVec3, kind: BodyKind, classes: ClassComposition) -> Self {
        let composition = classes.to_element_composition();
        Self {
            pos,
            vel,
            kind,
            composition,
            classes,
            satellites: Vec::new(),
        }
    }

    /// Deposit `mass` (M☉) of a condensation class — **the only way to add bulk
    /// material**. Updates the class breakdown and rebuilds the element vector from
    /// it, so the two representations can never drift.
    pub fn deposit(&mut self, class: CondensationClass, mass: f64) {
        self.classes.add_class(class, mass);
        self.resync_composition();
    }

    /// Fold another class composition into this body (e.g. a merge), keeping the
    /// element vector consistent.
    pub fn deposit_composition(&mut self, other: &ClassComposition) {
        self.classes.add(other);
        self.resync_composition();
    }

    /// **Absorb** material into the body — the formation-facing name for the
    /// *insert-into-body* half of an accretion transfer. The companion to
    /// [`Cloud::draw_band`](super::cloud::Cloud::draw_band): `body.absorb(&cloud.draw_band(..))`
    /// moves mass from the cloud into the body, conserved. Equivalent to
    /// [`Self::deposit_composition`]; named for the accounting it expresses.
    pub fn absorb(&mut self, material: &ClassComposition) {
        self.deposit_composition(material);
    }

    /// Remove `mass` (M☉) from the outermost classes first (a collision peeling
    /// envelope/mantle before the core), returning what was removed. Keeps the
    /// element vector in sync. See [`ClassComposition::strip_outermost`].
    pub fn strip(&mut self, mass: f64) -> ClassComposition {
        let removed = self.classes.strip_outermost(mass);
        self.resync_composition();
        removed
    }

    /// Re-derive the element vector from the class breakdown — *class drives
    /// elements*. Called by every mutator above, so the conserved element ledger
    /// always reflects exactly what the body has absorbed.
    fn resync_composition(&mut self) {
        self.composition = self.classes.to_element_composition();
    }

    // ----- Field 1: composition (stored) ----------------------------------------

    /// Total mass (M☉) — gravity, radius, density and pressure all key off this.
    pub fn mass(&self) -> f64 {
        self.classes.total()
    }

    /// The element holding the most mass, if any (for an element-tint).
    pub fn dominant_element(&self) -> Option<ElementId> {
        self.composition.dominant()
    }

    /// The condensation class holding the most mass — the body's coarse "type"
    /// (rocky / icy / gas), for colour and classification.
    pub fn dominant_class(&self) -> Option<CondensationClass> {
        self.classes.dominant()
    }

    /// Bulk volume (cm³) of the differentiated interior (see
    /// [`ClassComposition::volume_cm3`]).
    pub fn volume_cm3(&self) -> f64 {
        self.classes.volume_cm3()
    }

    // ----- Field 2: density (derived) -------------------------------------------

    /// Bulk density (g/cm³) of the differentiated body — distinguishes an iron
    /// world from an ice world. `0.0` for an empty body.
    pub fn density_g_cm3(&self) -> f64 {
        self.classes.density()
    }

    /// Physical radius (AU) from the differentiated volume. `0.0` if the body has
    /// no resolvable volume.
    pub fn physical_radius(&self) -> f64 {
        let v = self.volume_cm3();
        if v <= 0.0 {
            return 0.0;
        }
        let r_cm = (3.0 * v / (4.0 * std::f64::consts::PI)).cbrt();
        r_cm / AU_CM
    }

    // ----- Field 3: gravity (derived) -------------------------------------------

    /// Surface gravity (m/s²): `G·M / R²`, in SI for a human-meaningful number (the
    /// value that drives surface expression — e.g. a gas giant's band count).
    /// `0.0` if the radius is unresolvable. Orbital/relational gravity uses the
    /// AU/yr/M☉ system instead — see [`Self::mu`] / [`Self::orbital_elements`].
    pub fn surface_gravity_si(&self) -> f64 {
        let r_au = self.physical_radius();
        if r_au <= 0.0 {
            return 0.0;
        }
        let r_m = r_au * AU_M;
        G_SI * (self.mass() * M_SUN_KG) / (r_m * r_m)
    }

    // ----- Field 4: pressure (derived) ------------------------------------------

    /// Estimated central pressure (GPa) for a self-gravitating sphere of uniform
    /// density: `P_c = 3·G·M² / (8π·R⁴)`. An order-of-magnitude estimate (Earth's
    /// centre is ~360 GPa) — enough to drive the "gas giant interior behaves like a
    /// fluid/water-world" contract (spec §7), the field that makes pressure a
    /// material-expression driver. `0.0` if the radius is unresolvable; a
    /// depth-resolved profile is a later refinement.
    pub fn central_pressure_gpa(&self) -> f64 {
        let r_au = self.physical_radius();
        if r_au <= 0.0 {
            return 0.0;
        }
        let r_m = r_au * AU_M;
        let m_kg = self.mass() * M_SUN_KG;
        let p_pa = 3.0 * G_SI * m_kg * m_kg / (8.0 * std::f64::consts::PI * r_m.powi(4));
        p_pa / 1.0e9
    }

    // ----- Orbital state (relative to the parent) -------------------------------

    /// Distance from the parent (AU) — heliocentric for a planet, planetocentric
    /// for a moon.
    pub fn orbital_radius(&self) -> f64 {
        self.pos.length()
    }

    /// The standard gravitational parameter `μ = G·(M_parent + m)` (AU³/yr²) for
    /// this body orbiting a parent of mass `parent_mass` (M☉) — the one input the
    /// orbital methods need from the tree.
    pub fn mu(&self, parent_mass: f64) -> f64 {
        G * (parent_mass + self.mass())
    }

    /// Specific orbital energy (AU²/yr²) about `parent_mass`. Negative → bound,
    /// positive → unbound.
    pub fn specific_energy(&self, parent_mass: f64) -> f64 {
        let r = self.pos.length();
        if r <= 0.0 {
            return 0.0;
        }
        0.5 * self.vel.length_squared() - self.mu(parent_mass) / r
    }

    /// Whether the body is gravitationally bound to its parent (elliptical orbit,
    /// not a hyperbolic escape).
    pub fn is_bound(&self, parent_mass: f64) -> bool {
        self.specific_energy(parent_mass) < 0.0
    }

    /// Orbital semi-major axis (AU) and eccentricity about `parent_mass`, from the
    /// relative state vector. For an unbound (hyperbolic) orbit `a` is negative and
    /// `e ≥ 1`; for a degenerate state both are `0`.
    pub fn orbital_elements(&self, parent_mass: f64) -> (f64, f64) {
        let mu = self.mu(parent_mass);
        let r = self.pos.length();
        if r <= 0.0 || mu <= 0.0 {
            return (0.0, 0.0);
        }
        let v2 = self.vel.length_squared();
        let energy = 0.5 * v2 - mu / r;
        let a = if energy.abs() < f64::EPSILON {
            f64::INFINITY
        } else {
            -mu / (2.0 * energy)
        };
        // Eccentricity vector: ((v² − μ/r)·r − (r·v)·v) / μ.
        let e_vec = ((v2 - mu / r) * self.pos - self.pos.dot(self.vel) * self.vel) / mu;
        (a, e_vec.length())
    }

    /// Orbital period (yr) for a bound orbit about `parent_mass`; `None` if unbound.
    pub fn period(&self, parent_mass: f64) -> Option<f64> {
        let (a, _) = self.orbital_elements(parent_mass);
        if a.is_finite() && a > 0.0 {
            Some(std::f64::consts::TAU * (a * a * a / self.mu(parent_mass)).sqrt())
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::units::M_EARTH;

    /// A roughly Earth-mass rock+iron body on a circular orbit at `a_au` around a
    /// 1 M☉ star (68% silicate, 32% metal — a differentiated terrestrial mix).
    fn earthlike(a_au: f64) -> Body {
        let mut classes = ClassComposition::of(CondensationClass::Silicate, 0.68 * M_EARTH);
        classes.add_class(CondensationClass::Metal, 0.32 * M_EARTH);
        let pos = DVec3::new(a_au, 0.0, 0.0);
        let v = (G * (1.0 + classes.total()) / a_au).sqrt();
        Body::from_classes(pos, DVec3::new(0.0, 0.0, v), BodyKind::Protoplanet, classes)
    }

    #[test]
    fn mass_is_the_composition_total() {
        let b = earthlike(1.0);
        assert!((b.mass() - M_EARTH).abs() < 1e-12);
        // The two compositions carry the same total mass (sync invariant).
        assert!((b.composition.total() - b.classes.total()).abs() < 1e-12);
    }

    #[test]
    fn deposit_and_strip_keep_the_two_compositions_in_sync() {
        let mut b = Body::new(DVec3::ZERO, DVec3::ZERO, BodyKind::Protoplanet);
        b.deposit(CondensationClass::Silicate, 2.0);
        b.deposit(CondensationClass::Metal, 1.0);
        b.deposit(CondensationClass::Gas, 0.5);
        assert!((b.composition.total() - b.classes.total()).abs() < 1e-9, "sync after deposits");
        assert!((b.classes.total() - 3.5).abs() < 1e-12);
        let removed = b.strip(0.5); // peels the gas envelope first
        assert!((removed.total() - 0.5).abs() < 1e-12);
        assert_eq!(b.classes.get(CondensationClass::Gas), 0.0);
        assert!((b.composition.total() - b.classes.total()).abs() < 1e-9, "sync after strip");
    }

    #[test]
    fn circular_orbit_recovers_its_radius_and_period() {
        let b = earthlike(1.0);
        let (a, e) = b.orbital_elements(1.0);
        assert!((a - 1.0).abs() < 1e-6, "a ≈ 1 AU, got {a}");
        assert!(e < 1e-6, "near-circular, e = {e}");
        let p = b.period(1.0).unwrap();
        assert!((p - 1.0).abs() < 1e-3, "period ≈ 1 yr, got {p}");
        assert!(b.is_bound(1.0));
    }

    #[test]
    fn unbound_body_reads_as_unbound() {
        let mut b = earthlike(1.0);
        b.vel *= 2.0; // well above escape
        assert!(!b.is_bound(1.0));
        let (a, e) = b.orbital_elements(1.0);
        assert!(a < 0.0, "hyperbolic a negative, got {a}");
        assert!(e >= 1.0, "hyperbolic e ≥ 1, got {e}");
        assert!(b.period(1.0).is_none());
    }

    #[test]
    fn earth_radius_and_gravity_land_near_reality() {
        let b = earthlike(1.0);
        // Earth's radius is ~4.26e-5 AU; a differentiated rock+iron body lands close.
        let r = b.physical_radius();
        assert!(r > 3.0e-5 && r < 6.0e-5, "earthlike radius {r} AU off");
        // Surface gravity within a factor of ~2 of Earth's 9.8 m/s².
        let g = b.surface_gravity_si();
        assert!(g > 5.0 && g < 20.0, "earthlike gravity {g} m/s² off");
        // Central pressure positive and in a plausible 100s-of-GPa range.
        let p = b.central_pressure_gpa();
        assert!(p > 50.0 && p < 2000.0, "earthlike central pressure {p} GPa off");
    }

    #[test]
    fn iron_world_is_denser_than_an_ice_world() {
        let iron = Body::from_classes(
            DVec3::ZERO,
            DVec3::ZERO,
            BodyKind::Protoplanet,
            ClassComposition::of(CondensationClass::Metal, 1.0e-6),
        );
        let icy = Body::from_classes(
            DVec3::ZERO,
            DVec3::ZERO,
            BodyKind::Protoplanet,
            ClassComposition::of(CondensationClass::Ice, 1.0e-6),
        );
        assert!(iron.density_g_cm3() > icy.density_g_cm3());
        assert!(iron.physical_radius() < icy.physical_radius(), "denser → smaller for equal mass");
    }

    #[test]
    fn empty_body_has_no_resolvable_fields() {
        let b = Body::new(DVec3::ZERO, DVec3::ZERO, BodyKind::Debris);
        assert_eq!(b.mass(), 0.0);
        assert_eq!(b.physical_radius(), 0.0);
        assert_eq!(b.density_g_cm3(), 0.0);
        assert_eq!(b.surface_gravity_si(), 0.0);
        assert_eq!(b.central_pressure_gpa(), 0.0);
    }
}
