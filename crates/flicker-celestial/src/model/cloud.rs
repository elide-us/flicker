//! [`Cloud`] — the system's material reservoir, as **pure data accounting**.
//!
//! This is the "system has the material; the body aggregates it by absorbing the
//! cloud" half of formation (the user's clarification). It is **not** a visual
//! object — the dust cloud's *look* is the renderer's artful volumetric pass; this
//! structure exists only so body growth can be **conservation-accounted**: mass is
//! *removed from the cloud* and *inserted into a body*, never duplicated, never
//! invented.
//!
//! It replaces the gap in the old formation path: there the disk was a purely
//! **analytic** field (`solid_surface_density(r) × composition_fractions(r)`) that
//! embryos were *integrated out of* — nothing the cloud actually *lost*, so
//! leftovers, belts and conservation weren't real. Materialising that statistical
//! field into a discrete, conserved [`Cloud`] is what lets a forming body draw the
//! cloud down and leave a remainder (the seed of belts/rings — spec §5).
//!
//! # Why rings (radial bins)
//!
//! The accounting — not any visual — needs radial resolution: a material's
//! condensation class is set by its disk radius (metal/rock inside the snow line,
//! ice past it), and a forming body feeds from a radial **band** around its orbit.
//! So the reservoir is a set of concentric [`CloudRing`]s, each holding a conserved
//! [`ClassComposition`]. Azimuthal structure is unnecessary: it would only matter
//! for a look this structure deliberately does not own.
//!
//! The *materialisation* (turning the analytic disk model into rings) and the
//! *feeding policy* (which body draws how much) are **formation** concerns
//! (spec §5) that will populate and drive this reservoir; this module supplies the
//! conserved substrate and the transfer primitive they build on.

use super::condensation::ClassComposition;

/// One concentric annulus of the cloud, holding the conserved material available
/// between `inner` and `outer` (AU). The [`ClassComposition`] is the condensation
/// classes condensed at this radius (so it carries both the bulk-density truth and,
/// via projection, the element distribution). `inner < outer`.
#[derive(Clone, Debug)]
pub struct CloudRing {
    /// Inner radius (AU).
    pub inner: f64,
    /// Outer radius (AU).
    pub outer: f64,
    /// The material available in this ring — conserved; drawn down as bodies absorb.
    pub material: ClassComposition,
}

impl CloudRing {
    /// A ring of the given radial extent and material.
    pub fn new(inner: f64, outer: f64, material: ClassComposition) -> Self {
        Self { inner, outer, material }
    }

    /// Material mass still available in this ring (M☉).
    pub fn mass(&self) -> f64 {
        self.material.total()
    }

    /// Whether this ring overlaps the radial band `[inner, outer]`.
    pub fn overlaps(&self, inner: f64, outer: f64) -> bool {
        self.inner < outer && self.outer > inner
    }

    /// Draw up to `amount` (M☉) of this ring's material, proportionally across its
    /// classes, returning what was removed (conservation-safe). See
    /// [`ClassComposition::take_mass`].
    pub fn take_mass(&mut self, amount: f64) -> ClassComposition {
        self.material.take_mass(amount)
    }
}

/// The system's material reservoir — a set of concentric [`CloudRing`]s. A pure
/// accounting structure: the conserved source bodies draw their mass from.
#[derive(Clone, Debug, Default)]
pub struct Cloud {
    /// The rings, conventionally ordered inner→outer (not required for correctness).
    pub rings: Vec<CloudRing>,
}

impl Cloud {
    /// An empty cloud.
    pub fn new() -> Self {
        Self::default()
    }

    /// A cloud from an explicit set of rings.
    pub fn from_rings(rings: Vec<CloudRing>) -> Self {
        Self { rings }
    }

    /// Total material mass still in the cloud (M☉).
    pub fn total_mass(&self) -> f64 {
        self.rings.iter().map(CloudRing::mass).sum()
    }

    /// Total material still in the cloud, as one class composition (the sum of the
    /// rings) — e.g. to see what the system has left after formation.
    pub fn total_composition(&self) -> ClassComposition {
        let mut acc = ClassComposition::new();
        for ring in &self.rings {
            acc.add(&ring.material);
        }
        acc
    }

    /// Material mass available within the radial band `[inner, outer]` (M☉) — the
    /// reach a forming body could feed from.
    pub fn mass_in_band(&self, inner: f64, outer: f64) -> f64 {
        self.rings
            .iter()
            .filter(|r| r.overlaps(inner, outer))
            .map(CloudRing::mass)
            .sum()
    }

    /// **Draw** up to `amount` (M☉) from the rings overlapping the band
    /// `[inner, outer]`, removing it proportionally to each overlapping ring's
    /// available mass and returning the total removed. This is the *remove-from-
    /// cloud* half of an accretion transfer; the caller (formation) hands the result
    /// to [`Body::absorb`](super::body::Body::absorb) for the *insert-into-body*
    /// half. Conservation-safe — the returned composition is exactly what left the
    /// rings, and never exceeds what they held.
    pub fn draw_band(&mut self, inner: f64, outer: f64, amount: f64) -> ClassComposition {
        if amount <= 0.0 {
            return ClassComposition::new();
        }
        let available = self.mass_in_band(inner, outer);
        if available <= 0.0 {
            return ClassComposition::new();
        }
        // The fraction of each overlapping ring to sweep up (≤ 1 when the band holds
        // more than asked); proportional draw keeps every ring's mix representative.
        let fraction = (amount / available).min(1.0);
        let mut drawn = ClassComposition::new();
        for ring in self.rings.iter_mut().filter(|r| r.overlaps(inner, outer)) {
            drawn.add(&ring.material.take_fraction(fraction));
        }
        drawn
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::body::{Body, BodyKind};
    use crate::model::condensation::CondensationClass;
    use glam::DVec3;

    fn ring(inner: f64, outer: f64, class: CondensationClass, mass: f64) -> CloudRing {
        CloudRing::new(inner, outer, ClassComposition::of(class, mass))
    }

    fn rocky_cloud() -> Cloud {
        Cloud::from_rings(vec![
            ring(0.3, 1.0, CondensationClass::Metal, 4.0e-6),
            ring(1.0, 2.7, CondensationClass::Silicate, 9.0e-6),
            ring(2.7, 8.0, CondensationClass::Ice, 30.0e-6), // past the snow line
        ])
    }

    #[test]
    fn totals_sum_the_rings() {
        let cloud = rocky_cloud();
        assert!((cloud.total_mass() - 43.0e-6).abs() < 1e-15);
        assert!((cloud.total_composition().total() - cloud.total_mass()).abs() < 1e-15);
        // Only the inner two rings overlap the terrestrial band.
        assert!((cloud.mass_in_band(0.5, 1.5) - 13.0e-6).abs() < 1e-15);
    }

    #[test]
    fn drawing_into_a_body_conserves_mass() {
        let mut cloud = rocky_cloud();
        let before = cloud.total_mass();
        let mut body = Body::new(DVec3::new(1.2, 0.0, 0.0), DVec3::ZERO, BodyKind::Protoplanet);

        // A body in the terrestrial band sweeps up some of the available material.
        let want = 5.0e-6;
        let drawn = cloud.draw_band(0.8, 1.4, want);
        body.absorb(&drawn);

        // The body got exactly what left the cloud, and total mass is conserved.
        assert!((drawn.total() - want).abs() < 1e-12, "drew the requested mass");
        assert!((body.mass() - drawn.total()).abs() < 1e-12, "body got exactly the draw");
        assert!((cloud.total_mass() + body.mass() - before).abs() < 1e-12, "mass conserved");
        // It pulled from both overlapping inner rings (metal + silicate), so the body
        // is a rock+iron mix — the accounting carried the class character across.
        assert!(body.classes.get(CondensationClass::Metal) > 0.0);
        assert!(body.classes.get(CondensationClass::Silicate) > 0.0);
        assert_eq!(body.classes.get(CondensationClass::Ice), 0.0, "snow-line ring untouched");
    }

    #[test]
    fn a_draw_larger_than_the_band_takes_only_what_is_there() {
        let mut cloud = rocky_cloud();
        let in_band = cloud.mass_in_band(0.0, 1.0); // the metal ring only
        let drawn = cloud.draw_band(0.0, 1.0, 1.0); // ask for far more than exists
        assert!((drawn.total() - in_band).abs() < 1e-15, "clamped to available");
        assert!((cloud.mass_in_band(0.0, 1.0)).abs() < 1e-15, "ring emptied, not over-drawn");
    }
}
