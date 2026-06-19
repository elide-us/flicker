//! The habitability verdict — a *classification* of a formed protoplanet against
//! Earth-analogue criteria. It only reads the simulation's output; it does not feed
//! back into how systems form. Whether many, few, or no worlds pass is an **emergent**
//! property of the formation physics, not a target this code (or the disk parameters)
//! aims for — and the dominant reason most planets fail is simply that the habitable
//! zone is a narrow annulus most orbits miss.
//!
//! A world qualifies if it sits in the habitable zone, is roughly Earth-massed, on a
//! near-circular orbit, is a *rocky* body (not a gas giant, ice world, or stripped iron
//! core), and carries surface-water-scale volatiles. The thresholds below are
//! habitability-science estimates, each justified at its definition.

use crate::astro::earth_masses;
use crate::body::{Body, BodyKind};
use crate::disk::{HZ_INNER, HZ_OUTER};
use crate::material::MaterialClass;

/// Minimum mass (M⊕) to plausibly retain a substantial atmosphere and sustain plate
/// tectonics / a magnetic dynamo over gigayears — Mars (0.107 M⊕) is below it; ~0.3 is
/// a common rough floor.
const MASS_MIN: f64 = 0.3;
/// Maximum mass (M⊕) before a world tends to hold a thick H/He envelope and become a
/// mini-Neptune rather than a rocky planet (the radius valley sits near ~1.5–2 R⊕).
const MASS_MAX: f64 = 3.0;
/// Above this orbital eccentricity the insolation swings over an orbit are large enough
/// to threaten a stable surface climate.
const E_MAX: f64 = 0.2;
/// Minimum water (ice-class mass fraction) for surface seas. Earth's water is ~0.02–0.1 %
/// of its mass, so even a trace clears a habitable threshold.
const WATER_MIN: f64 = 1.0e-4;
/// Above a few percent water by mass a world is a deep global ocean with no exposed land
/// (a "water world"), generally taken to lack the land/ocean/weathering cycle of Earth.
const WATER_MAX: f64 = 0.05;

/// The verdict for one body: playable, or the reasons it isn't.
#[derive(Clone, Debug, Default)]
pub struct Verdict {
    pub playable: bool,
    pub reasons: Vec<&'static str>,
}

impl Verdict {
    /// A one-line summary for the HUD.
    pub fn summary(&self) -> String {
        if self.playable {
            "PLAYABLE".to_string()
        } else {
            format!("no: {}", self.reasons.join(", "))
        }
    }
}

/// Assess a formed body against the habitability criteria.
pub fn assess(body: &Body) -> Verdict {
    let mut reasons = Vec::new();

    let (a, e) = body.orbital_elements();
    let mass_e = earth_masses(body.mass);
    let water = body.comp.water_fraction();
    let dominant = body.dominant();

    // Orbit: bound, in the habitable zone, low eccentricity.
    if !body.is_bound() || a <= 0.0 {
        reasons.push("unbound orbit");
    } else if a < HZ_INNER {
        reasons.push("too hot (inside HZ)");
    } else if a > HZ_OUTER {
        reasons.push("too cold (beyond HZ)");
    }
    if e > E_MAX {
        reasons.push("eccentric orbit");
    }

    // Mass.
    if mass_e < MASS_MIN {
        reasons.push("too small");
    } else if mass_e > MASS_MAX {
        reasons.push("too massive");
    }

    // Type: a rocky world, not a giant / ice world / stripped iron core.
    if body.kind == BodyKind::Giant {
        reasons.push("gas giant");
    } else if body.kind == BodyKind::Debris {
        reasons.push("collision debris");
    }
    match dominant {
        Some(MaterialClass::Silicate) => {}
        Some(MaterialClass::Metal) => reasons.push("stripped iron core"),
        Some(MaterialClass::Ice) => reasons.push("ice world"),
        Some(MaterialClass::Gas) => reasons.push("gas-dominated"),
        Some(MaterialClass::Carbon) => reasons.push("carbon world"),
        None => reasons.push("no composition"),
    }

    // Water: enough for seas, not so much it drowns.
    if water < WATER_MIN {
        reasons.push("no surface water");
    } else if water > WATER_MAX {
        reasons.push("drowned (ice world)");
    }

    Verdict {
        playable: reasons.is_empty(),
        reasons,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::astro::M_EARTH;
    use crate::body::circular_speed;
    use crate::material::Composition;
    use glam::DVec3;

    /// A rocky body of `mass_e` Earth masses at `a` AU on a circular orbit, with a
    /// trace of ice (`water` mass fraction).
    fn world(mass_e: f64, a: f64, water: f64) -> Body {
        let m = mass_e * M_EARTH;
        let mut comp = Composition::of(MaterialClass::Silicate, m * (1.0 - water - 0.3));
        comp.add(&Composition::of(MaterialClass::Metal, m * 0.3));
        comp.add(&Composition::of(MaterialClass::Ice, m * water));
        let pos = DVec3::new(a, 0.0, 0.0);
        let vel = DVec3::new(0.0, 0.0, circular_speed(a, comp.total()));
        Body::new(pos, vel, comp, BodyKind::Protoplanet)
    }

    #[test]
    fn earthlike_world_is_playable() {
        let v = assess(&world(1.0, 1.05, 0.002));
        assert!(v.playable, "should be playable: {:?}", v.reasons);
    }

    #[test]
    fn hot_dry_world_is_not() {
        let v = assess(&world(1.0, 0.4, 0.0));
        assert!(!v.playable);
        assert!(v.reasons.contains(&"too hot (inside HZ)"));
        assert!(v.reasons.contains(&"no surface water"));
    }

    #[test]
    fn giant_is_not_playable() {
        let mut b = world(2.0, 1.1, 0.001);
        b.kind = BodyKind::Giant;
        let v = assess(&b);
        assert!(!v.playable);
        assert!(v.reasons.contains(&"gas giant"));
    }
}
