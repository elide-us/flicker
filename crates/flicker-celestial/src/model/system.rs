//! [`System`] — a whole star system as a tree, plus the derived
//! [`Classification`] of each body.
//!
//! A system is just its **root body, the star**; everything else hangs off it as
//! [`Satellite`]s (planets, belts, comets), which recurse (a planet's moons, a
//! moon's submoons). `System` is the thin wrapper that walks that tree with the
//! parent-mass context the orbital math needs, and labels bodies the IAU way.

use super::body::Body;
use super::satellite::Satellite;

/// The contextual label of a body — what it *is* in its setting, as opposed to the
/// intrinsic [`BodyKind`](super::body::BodyKind). Derived from the tree (a body's
/// parent, siblings and orbit), never stored, because the same physical body reads
/// as a "planet" alone in its lane and a "dwarf" in a crowded belt.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Classification {
    /// The central fusing star (the root).
    Star,
    /// A gas giant (captured an envelope).
    GasGiant,
    /// A body orbiting the star that has **cleared its orbital band** (IAU planet).
    Planet,
    /// A body orbiting the star that shares its band with comparable mass (IAU
    /// dwarf / belt object).
    DwarfPlanet,
    /// A body bound to a non-star parent (a moon / submoon).
    Moon,
    /// A small body on a highly eccentric, sweeping orbit.
    Comet,
    /// A collision fragment.
    Debris,
}

/// Eccentricity at or above which a small body reads as a [`Comet`](Classification::Comet)
/// rather than a planet/dwarf — a *placeholder* threshold to tune once formation
/// emits comets (flagged in the spec's open questions).
pub const COMET_MIN_ECCENTRICITY: f64 = 0.4;

/// Neighbourhood half-width as a fraction of a body's semi-major axis, for the IAU
/// "cleared its band" test (ported from the formation sim).
const BAND_FRACTION: f64 = 0.15;

/// Has `siblings[idx]` **cleared its orbital neighbourhood** — does it outweigh the
/// sum of the other (non-giant) siblings sharing its band (within ±[`BAND_FRACTION`]
/// of its semi-major axis)? The IAU criterion separating a planet from a dwarf.
/// `parent_mass` (M☉) sets the orbital μ for every sibling. Giants count as cleared
/// by definition.
pub fn cleared_neighborhood(parent_mass: f64, siblings: &[&Body], idx: usize) -> bool {
    let body = siblings[idx];
    if body.kind.is_giant() {
        return true;
    }
    let (a, _) = body.orbital_elements(parent_mass);
    if a <= 0.0 || a.is_nan() {
        return false;
    }
    let band = BAND_FRACTION * a;
    let mut neighbour_mass = 0.0;
    for (j, o) in siblings.iter().enumerate() {
        if j == idx || o.kind.is_giant() {
            continue;
        }
        let (ao, _) = o.orbital_elements(parent_mass);
        if ao > 0.0 && (ao - a).abs() < band {
            neighbour_mass += o.mass();
        }
    }
    body.mass() > neighbour_mass
}

/// Classify `siblings[idx]` given its parent context: `parent_mass` (M☉) and
/// whether the parent is the star. Bodies bound to a planet are moons; bodies
/// orbiting the star are giants / comets / planets / dwarfs.
pub fn classify(parent_mass: f64, parent_is_star: bool, siblings: &[&Body], idx: usize) -> Classification {
    use super::body::BodyKind;
    let body = siblings[idx];
    match body.kind {
        BodyKind::Star => Classification::Star,
        BodyKind::Giant => Classification::GasGiant,
        BodyKind::Debris => Classification::Debris,
        BodyKind::Protoplanet => {
            if !parent_is_star {
                return Classification::Moon;
            }
            let (_, e) = body.orbital_elements(parent_mass);
            if e >= COMET_MIN_ECCENTRICITY {
                Classification::Comet
            } else if cleared_neighborhood(parent_mass, siblings, idx) {
                Classification::Planet
            } else {
                Classification::DwarfPlanet
            }
        }
    }
}

/// A star system — its root body (the star) and the tree beneath it.
#[derive(Clone, Debug)]
pub struct System {
    /// The root node: the central star. Its satellites are the planets, belts and
    /// comets (which recurse into moons, rings, …).
    pub star: Body,
}

impl System {
    /// A system from its central star.
    pub fn new(star: Body) -> Self {
        Self { star }
    }

    /// The star's mass (M☉) — the μ source for every heliocentric orbit.
    pub fn star_mass(&self) -> f64 {
        self.star.mass()
    }

    /// Total mass of the whole tree (M☉) — every body plus every disc's material.
    pub fn total_mass(&self) -> f64 {
        fn walk(body: &Body, acc: &mut f64) {
            *acc += body.mass();
            for sat in &body.satellites {
                match sat {
                    Satellite::Body(child) => walk(child, acc),
                    Satellite::Disc(disc) => *acc += disc.mass(),
                }
            }
        }
        let mut acc = 0.0;
        walk(&self.star, &mut acc);
        acc
    }

    /// Visit every [`Body`] in the tree depth-first, with the **parent's mass**
    /// (M☉) and the depth (the star is depth 0, parent_mass 0.0). The parent mass
    /// is what the orbital methods need, so a consumer can compute orbits for the
    /// whole tree in one walk.
    pub fn for_each_body(&self, mut f: impl FnMut(&Body, f64, usize)) {
        fn walk<F: FnMut(&Body, f64, usize)>(body: &Body, parent_mass: f64, depth: usize, f: &mut F) {
            f(body, parent_mass, depth);
            let m = body.mass();
            for sat in &body.satellites {
                if let Satellite::Body(child) = sat {
                    walk(child, m, depth + 1, f);
                }
            }
        }
        walk(&self.star, 0.0, 0, &mut f);
    }

    /// Count the bodies in the tree (the star included).
    pub fn count_bodies(&self) -> usize {
        let mut n = 0;
        self.for_each_body(|_, _, _| n += 1);
        n
    }

    /// The star's direct child bodies (the planets / dwarfs / comets), in order —
    /// the sibling set the heliocentric classification compares within. Discs are
    /// excluded.
    pub fn solar_bodies(&self) -> Vec<&Body> {
        self.star
            .satellites
            .iter()
            .filter_map(Satellite::as_body)
            .collect()
    }

    /// Classify the `idx`-th solar body (a direct child of the star). `None` if out
    /// of range.
    pub fn classify_solar_body(&self, idx: usize) -> Option<Classification> {
        let siblings = self.solar_bodies();
        if idx >= siblings.len() {
            return None;
        }
        Some(classify(self.star_mass(), true, &siblings, idx))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::body::{Body, BodyKind};
    use crate::model::condensation::{ClassComposition, CondensationClass};
    use crate::model::satellite::Satellite;
    use crate::units::{G, M_EARTH};
    use glam::DVec3;

    /// A circular-orbit body at `a_au` of `m` solar masses, mostly silicate+metal.
    fn body_at(a_au: f64, m: f64, kind: BodyKind) -> Body {
        let mut classes = ClassComposition::of(CondensationClass::Silicate, 0.68 * m);
        classes.add_class(CondensationClass::Metal, 0.32 * m);
        let v = (G * (1.0 + m) / a_au).sqrt();
        Body::from_classes(DVec3::new(a_au, 0.0, 0.0), DVec3::new(0.0, 0.0, v), kind, classes)
    }

    fn sun() -> Body {
        Body::from_classes(
            DVec3::ZERO,
            DVec3::ZERO,
            BodyKind::Star,
            ClassComposition::of(CondensationClass::Gas, 1.0),
        )
    }

    #[test]
    fn tree_walk_counts_bodies_and_passes_parent_mass() {
        let mut star = sun();
        let mut earth = body_at(1.0, M_EARTH, BodyKind::Protoplanet);
        // A moon of Earth (planetocentric state; mass tiny).
        let moon = Body::from_classes(
            DVec3::new(2.5e-3, 0.0, 0.0),
            DVec3::new(0.0, 0.0, 0.2),
            BodyKind::Protoplanet,
            ClassComposition::of(CondensationClass::Silicate, 0.012 * M_EARTH),
        );
        earth.satellites.push(Satellite::Body(moon));
        star.satellites.push(Satellite::Body(earth));
        let sys = System::new(star);

        assert_eq!(sys.count_bodies(), 3, "star + earth + moon");

        // The walk hands each body its parent's mass: the star sees 0, Earth sees
        // the star's mass, the moon sees Earth's mass.
        let star_m = sys.star_mass();
        let mut seen = Vec::new();
        sys.for_each_body(|b, parent_mass, depth| seen.push((b.kind, parent_mass, depth)));
        assert_eq!(seen[0].2, 0, "star at depth 0");
        assert!((seen[1].1 - star_m).abs() < 1e-12, "planet's parent mass is the star");
        assert_eq!(seen[2].2, 2, "moon at depth 2");
        assert!((seen[2].1 - M_EARTH).abs() < 1e-6, "moon's parent mass is ~Earth");
    }

    #[test]
    fn lone_planet_clears_its_lane_but_a_crowd_does_not() {
        // One Earth alone at 1 AU → cleared → planet.
        let mut star = sun();
        star.satellites.push(Satellite::Body(body_at(1.0, M_EARTH, BodyKind::Protoplanet)));
        let lone = System::new(star);
        assert_eq!(lone.classify_solar_body(0), Some(Classification::Planet));

        // Three equal masses crammed in one band → none clears → all dwarfs.
        let mut star = sun();
        for a in [0.98, 1.0, 1.02] {
            star.satellites.push(Satellite::Body(body_at(a, M_EARTH, BodyKind::Protoplanet)));
        }
        let crowd = System::new(star);
        for i in 0..3 {
            assert_eq!(crowd.classify_solar_body(i), Some(Classification::DwarfPlanet), "body {i}");
        }
    }

    #[test]
    fn giant_classifies_as_a_giant_and_clears_by_definition() {
        let mut star = sun();
        star.satellites.push(Satellite::Body(body_at(5.0, 300.0 * M_EARTH, BodyKind::Giant)));
        let sys = System::new(star);
        assert_eq!(sys.classify_solar_body(0), Some(Classification::GasGiant));
    }

    #[test]
    fn eccentric_small_body_reads_as_a_comet() {
        let mut star = sun();
        // Build a high-eccentricity orbit: a slow tangential speed at perihelion-ish
        // gives a stretched ellipse (e well above the comet threshold).
        let m = 1.0e-4 * M_EARTH;
        let a_au = 1.0;
        let v_circ = (G * (1.0 + m) / a_au).sqrt();
        let comet = Body::from_classes(
            DVec3::new(a_au, 0.0, 0.0),
            DVec3::new(0.0, 0.0, v_circ * 0.5), // half circular speed → very eccentric, bound
            BodyKind::Protoplanet,
            ClassComposition::of(CondensationClass::Ice, m),
        );
        let (_, e) = comet.orbital_elements(1.0);
        assert!(e >= COMET_MIN_ECCENTRICITY, "set up a high-e orbit, got e={e}");
        star.satellites.push(Satellite::Body(comet));
        let sys = System::new(star);
        assert_eq!(sys.classify_solar_body(0), Some(Classification::Comet));
    }

    #[test]
    fn a_bound_child_of_a_planet_is_a_moon() {
        let mut earth = body_at(1.0, M_EARTH, BodyKind::Protoplanet);
        let moon = Body::from_classes(
            DVec3::new(2.5e-3, 0.0, 0.0),
            DVec3::new(0.0, 0.0, 0.2),
            BodyKind::Protoplanet,
            ClassComposition::of(CondensationClass::Silicate, 0.012 * M_EARTH),
        );
        earth.satellites.push(Satellite::Body(moon));
        // Classify the moon directly via the helper (parent is not the star).
        let siblings: Vec<&Body> = earth.satellites.iter().filter_map(Satellite::as_body).collect();
        assert_eq!(classify(earth.mass(), false, &siblings, 0), Classification::Moon);
    }
}
