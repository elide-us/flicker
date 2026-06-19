//! **Formation** — the transform from a nebula to a star system. This slice builds
//! the foundation the spec's formation work (§5) stands on: **materialising the
//! analytic disk field into a conserved [`Cloud`]** so body growth can be accounted
//! correctly (the user's "turn the statistical distribution into the actual
//! distribution" — the gap in the old POC, where embryos were *integrated out of* an
//! analytic field nothing ever lost).
//!
//! What's here:
//! - [`disk`] — the analytic disk physics ([`Nebula`], `Σ(r)`, composition-by-radius).
//! - [`materialize_cloud`] — discretise that field into the conserved reservoir.
//!
//! What's deliberately **not** here yet (the next, separate step — it is the one
//! genuinely creative decision, spec §11): **how bodies are seeded and how much of
//! the cloud they consume**. A full sweep (the old behaviour) leaves no remainder; a
//! *partial* / probabilistic consumption leaves belts, rings and comets (spec §5).
//! Which consumption model to use is a design call to make deliberately, so seeding
//! is left for that step. The transfer primitive it will build on already exists:
//! [`Cloud::draw_band`] (remove from cloud) → [`Body::absorb`] (insert into body),
//! conserved — demonstrated in the tests below.
//!
//! [`Cloud::draw_band`]: crate::model::Cloud::draw_band
//! [`Body::absorb`]: crate::model::Body::absorb

pub mod disk;

pub use disk::{
    annulus_solid_mass, class_composition_at, composition_fractions, random_supernova,
    solid_surface_density, Nebula, Rng, DISK_INNER, DISK_OUTER, SNOW_LINE,
};

use crate::model::{Cloud, CloudRing};

/// Default number of rings the disk is materialised into — the accounting granularity
/// (finer = closer to the analytic field, more rings to draw from). Not a visual
/// parameter; the cloud is never rendered.
pub const DEFAULT_CLOUD_RINGS: usize = 64;

/// **Materialise** a [`Nebula`]'s analytic solid field into a conserved [`Cloud`]: split
/// `[DISK_INNER, DISK_OUTER]` into `n_rings` concentric annuli and give each the solid
/// mass it actually holds ([`annulus_solid_mass`]), distributed into condensation
/// classes by the composition at its midpoint ([`class_composition_at`]).
///
/// The result is the *actual* distribution bodies draw down — total cloud mass equals
/// the disk's integrated solids, so every later [`Cloud::draw_band`](crate::model::Cloud::draw_band)
/// into a body is conservation-accounted. Empty rings (negligible mass) are dropped.
/// `n_rings` is clamped to at least 1.
pub fn materialize_cloud(nebula: &Nebula, n_rings: usize) -> Cloud {
    let n = n_rings.max(1);
    let sigma = nebula.solid_sigma();
    let dr = (DISK_OUTER - DISK_INNER) / n as f64;
    let mut rings = Vec::with_capacity(n);
    for i in 0..n {
        let r0 = DISK_INNER + i as f64 * dr;
        let r1 = r0 + dr;
        let mass = annulus_solid_mass(r0, r1, sigma);
        if mass <= 0.0 {
            continue;
        }
        let center = 0.5 * (r0 + r1);
        rings.push(CloudRing::new(r0, r1, class_composition_at(center, mass)));
    }
    Cloud::from_rings(rings)
}

/// [`materialize_cloud`] at the [`DEFAULT_CLOUD_RINGS`] granularity.
pub fn materialize_cloud_default(nebula: &Nebula) -> Cloud {
    materialize_cloud(nebula, DEFAULT_CLOUD_RINGS)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Body, BodyKind, CondensationClass};
    use glam::DVec3;

    #[test]
    fn materialised_cloud_conserves_the_disk_solids() {
        let nebula = Nebula::new(42, 0.6);
        let cloud = materialize_cloud(&nebula, 128);
        // The rings together hold (close to) the disk's whole integrated solid mass.
        let analytic = annulus_solid_mass(DISK_INNER, DISK_OUTER, nebula.solid_sigma());
        let materialised = cloud.total_mass();
        assert!(materialised > 0.0, "a non-empty cloud");
        let rel = (materialised - analytic).abs() / analytic;
        assert!(rel < 0.01, "materialised {materialised} vs analytic {analytic} (rel {rel})");
        // total_composition is just the sum of the rings.
        assert!((cloud.total_composition().total() - materialised).abs() < 1e-12);
    }

    #[test]
    fn the_inner_cloud_is_dry_and_the_outer_cloud_is_icy() {
        let cloud = materialize_cloud(&Nebula::new(7, 0.7), 64);
        let inner = cloud.rings.iter().find(|r| r.outer <= 1.0).expect("an inner ring");
        let outer = cloud.rings.iter().find(|r| r.inner >= SNOW_LINE + 1.0).expect("an outer ring");
        assert!(inner.material.get(CondensationClass::Ice) < 1e-9, "inner ring is dry");
        assert_eq!(outer.material.dominant(), Some(CondensationClass::Ice), "outer ring is icy");
    }

    #[test]
    fn a_body_absorbing_from_the_cloud_conserves_total_mass() {
        let nebula = Nebula::new(11, 0.8);
        let mut cloud = materialize_cloud(&nebula, 128);
        let before = cloud.total_mass();

        // A body in the terrestrial band sweeps up half of the material *in that band*
        // (drawing relative to the band, so the request is genuinely available).
        let mut body = Body::new(DVec3::new(1.0, 0.0, 0.0), DVec3::ZERO, BodyKind::Protoplanet);
        let want = cloud.mass_in_band(0.7, 1.3) * 0.5;
        assert!(want > 0.0, "the terrestrial band holds material");
        body.absorb(&cloud.draw_band(0.7, 1.3, want));

        assert!((body.mass() - want).abs() < 1e-12, "body got what it drew");
        assert!((cloud.total_mass() + body.mass() - before).abs() < 1e-12, "mass conserved");
        // The inner band is rock+metal, so the body is rocky; only the trace hydration
        // floor (hydrated silicates) inside the snow line, far from an icy world.
        assert_eq!(body.dominant_class(), Some(CondensationClass::Silicate));
        assert!(body.classes.water_fraction() < 0.02, "only trace water inside the snow line");
    }
}
