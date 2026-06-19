//! [`Satellite`] — the recursive tree edge — and [`Disc`], the ring/belt material
//! annulus.
//!
//! A [`Body`] owns a list of satellites. The spec names four satellite
//! *classifications* — body, ring, belt, comet — but only **two** are distinct
//! structures, because the spec also says to model the continuums, not the labels:
//!
//! - a **discrete body** (a moon, a submoon, or a **comet** — a comet is a body
//!   whose orbit is highly eccentric, not a separate type) → [`Satellite::Body`],
//!   a full recursive [`Body`];
//! - an **annulus of material** (a **ring** *or* a **belt** — two points on a
//!   density continuum, not distinct types) → [`Satellite::Disc`].
//!
//! The four classifications are recovered as derived tags:
//! [`Classification`](super::system::Classification) distinguishes moon vs comet
//! from a body's orbit; [`Disc::class`] distinguishes ring vs belt from the disc's
//! surface density.

use super::body::Body;
use super::condensation::ClassComposition;

/// A satellite bound to a parent body. A parent may hold any combination.
#[derive(Clone, Debug)]
pub enum Satellite {
    /// A discrete bound sub-body: a moon, a submoon, or a comet. A full recursive
    /// [`Body`] — its own satellites hang off it. Comet-vs-moon is a *derived*
    /// classification of the orbit (see
    /// [`Classification`](super::system::Classification)), not a separate variant.
    Body(Body),
    /// An annulus of material in (roughly) the parent's equatorial plane — a ring
    /// or a belt. Ring-vs-belt is a point on the [`Disc::class`] density continuum.
    Disc(Disc),
}

impl Satellite {
    /// The child body, if this satellite is one.
    pub fn as_body(&self) -> Option<&Body> {
        match self {
            Satellite::Body(b) => Some(b),
            Satellite::Disc(_) => None,
        }
    }

    /// The disc, if this satellite is one.
    pub fn as_disc(&self) -> Option<&Disc> {
        match self {
            Satellite::Disc(d) => Some(d),
            Satellite::Body(_) => None,
        }
    }
}

/// A radial gap in a [`Disc`] — the clear lane a resonance or a shepherd satellite
/// carves (Cassini-division style). Radii are in the disc's own units (AU,
/// parent-relative), `inner < outer`.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct DiscGap {
    /// Inner edge of the gap (AU).
    pub inner: f64,
    /// Outer edge of the gap (AU).
    pub outer: f64,
}

impl DiscGap {
    /// The radial width the gap clears (AU), clamped at zero.
    pub fn width(&self) -> f64 {
        (self.outer - self.inner).max(0.0)
    }
}

/// Where a [`Disc`] sits on the ring↔belt continuum. A *derived* tag — the
/// underlying data is one structure (see [`Disc::class`]).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum DiscClass {
    /// A dense, tightly-packed disc of fine material (a planetary ring).
    Ring,
    /// A sparse, populated annulus of small bodies (an asteroid / Kuiper belt).
    Belt,
}

/// An annulus of material orbiting a parent — a ring or a belt. The two are the
/// same structure at different **surface densities** (mass per unit annulus area):
/// Saturn's rings pack a high surface density of fine ice; the asteroid belt is a
/// sparse scatter of larger bodies. [`Self::class`] reads off that continuum.
///
/// Radii are parent-relative (AU). The material is a [`ClassComposition`] so a disc
/// inherits the same "icy vs rocky" character as a body — which drives both its
/// ring/belt tendency and (for rendering) its hue.
#[derive(Clone, Debug)]
pub struct Disc {
    /// Inner radius (AU, parent-relative).
    pub inner: f64,
    /// Outer radius (AU, parent-relative).
    pub outer: f64,
    /// Tilt of the disc plane off the parent's equatorial plane (radians).
    pub inclination: f64,
    /// What the disc is made of — drives surface density, ring/belt class, and hue.
    pub material: ClassComposition,
    /// Cleared lanes (resonance / shepherd-carved). Empty for an ungapped disc.
    pub gaps: Vec<DiscGap>,
}

/// Default surface-density threshold (M☉ / AU²) separating a ring from a belt — a
/// *placeholder* modeling constant to confirm/tune with real anchor values once
/// formation feeds discs (flagged in the spec as a "potential invariant"). Above
/// it a disc reads as a dense [`Ring`](DiscClass::Ring); below, a sparse
/// [`Belt`](DiscClass::Belt).
pub const RING_BELT_SURFACE_DENSITY: f64 = 1.0e-12;

impl Disc {
    /// Total material mass (M☉).
    pub fn mass(&self) -> f64 {
        self.material.total()
    }

    /// Area of the annulus (AU²), net of any gaps. `0.0` if degenerate
    /// (`outer ≤ inner`).
    pub fn area_au2(&self) -> f64 {
        if self.outer <= self.inner {
            return 0.0;
        }
        let ring_area = |a: f64, b: f64| std::f64::consts::PI * (b * b - a * a);
        let mut area = ring_area(self.inner, self.outer);
        for gap in &self.gaps {
            // Only the part of the gap that overlaps the annulus is removed.
            let lo = gap.inner.max(self.inner);
            let hi = gap.outer.min(self.outer);
            if hi > lo {
                area -= ring_area(lo, hi);
            }
        }
        area.max(0.0)
    }

    /// Surface mass density (M☉ / AU²) — the continuum axis between ring and belt.
    /// `0.0` for a degenerate annulus.
    pub fn surface_density(&self) -> f64 {
        let area = self.area_au2();
        if area <= 0.0 {
            0.0
        } else {
            self.mass() / area
        }
    }

    /// Ring vs belt by the given surface-density `threshold` (M☉ / AU²).
    pub fn class_with(&self, threshold: f64) -> DiscClass {
        if self.surface_density() >= threshold {
            DiscClass::Ring
        } else {
            DiscClass::Belt
        }
    }

    /// Ring vs belt at the default [`RING_BELT_SURFACE_DENSITY`] threshold.
    pub fn class(&self) -> DiscClass {
        self.class_with(RING_BELT_SURFACE_DENSITY)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::condensation::CondensationClass;

    fn disc(inner: f64, outer: f64, class: CondensationClass, mass: f64) -> Disc {
        Disc {
            inner,
            outer,
            inclination: 0.0,
            material: ClassComposition::of(class, mass),
            gaps: Vec::new(),
        }
    }

    #[test]
    fn dense_narrow_disc_out_densities_a_sparse_wide_one() {
        // Same mass; the tight annulus has a far higher surface density.
        let tight = disc(0.001, 0.0015, CondensationClass::Ice, 1.0e-12);
        let wide = disc(2.0, 4.0, CondensationClass::Silicate, 1.0e-12);
        assert!(
            tight.surface_density() > wide.surface_density(),
            "tight {} vs wide {}",
            tight.surface_density(),
            wide.surface_density(),
        );
    }

    #[test]
    fn class_follows_the_density_threshold() {
        let d = disc(0.001, 0.0015, CondensationClass::Ice, 1.0e-12);
        let sd = d.surface_density();
        assert_eq!(d.class_with(sd * 0.5), DiscClass::Ring, "above threshold → ring");
        assert_eq!(d.class_with(sd * 2.0), DiscClass::Belt, "below threshold → belt");
    }

    #[test]
    fn gaps_reduce_area_and_raise_surface_density() {
        let mut d = disc(1.0, 2.0, CondensationClass::Silicate, 1.0e-12);
        let dense_before = d.surface_density();
        d.gaps.push(DiscGap { inner: 1.4, outer: 1.6 });
        assert!(d.area_au2() < std::f64::consts::PI * (4.0 - 1.0), "gap removed some area");
        assert!(d.surface_density() > dense_before, "less area, same mass → denser");
    }

    #[test]
    fn degenerate_annulus_is_inert() {
        let d = disc(2.0, 2.0, CondensationClass::Ice, 1.0e-12);
        assert_eq!(d.area_au2(), 0.0);
        assert_eq!(d.surface_density(), 0.0);
    }
}
