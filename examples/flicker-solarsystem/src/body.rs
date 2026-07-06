//! A forming body — embryo, protoplanet, giant, or collision debris — as it moves
//! under the star and its peers.
//!
//! State is heliocentric position/velocity in `f64` (`glam::DVec3`), mass in solar
//! masses, and a [`Composition`]. Geometry (physical radius, density) comes from the
//! composition, so an iron-rich body is small and dense and an icy one large and
//! light — the same bookkeeping that makes collisions strip mantle before core.

use glam::DVec3;

use crate::astro::{AU_CM, G, M_STAR};
use crate::material::{Composition, MaterialClass};

/// How much collision radii are inflated over physical radii. Physical planetary
/// radii are ~10⁻⁵ AU — at that size, and over the compressed wall-clock we can
/// integrate (we cannot run the literal ~10⁷-orbit giant-impact phase), bodies
/// would essentially never touch. Inflating the *collision* cross-section is the
/// standard fast-accretion trick: it speeds up accretion so the phase completes in
/// a tractable number of steps while leaving the gravitational dynamics untouched.
/// (It is the main reason the displayed time is a scaled, cosmetic label.)
pub const COLLISION_INFLATION: f64 = 1200.0;

/// What a body is, for labelling and rendering.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum BodyKind {
    /// A rocky/icy protoplanet (the default — embryos and their merger products).
    Protoplanet,
    /// A gas giant: a core past the snow line that captured an H/He envelope.
    Giant,
    /// A collision fragment (erosion/disruption debris).
    Debris,
}

/// A captured satellite, bookkept on its parent body. Tracked separately from the parent
/// (it orbits the planet, it is *not* part of the planet's bulk composition / Epoch-1
/// export) and not integrated as its own N-body (its orbit around the planet is far
/// tighter than the global timestep resolves — a standard satellite-bookkeeping shortcut).
#[derive(Clone, Debug)]
pub struct Moon {
    pub mass: f64,
    pub comp: Composition,
}

/// One body in the simulation.
#[derive(Clone, Debug)]
pub struct Body {
    pub pos: DVec3,
    pub vel: DVec3,
    /// Mass in solar masses (equals `comp.total()`; moons are tracked separately).
    pub mass: f64,
    pub comp: Composition,
    pub kind: BodyKind,
    /// Satellites captured by this body (see [`Moon`]).
    pub moons: Vec<Moon>,
}

impl Body {
    /// A body from a composition and orbital state; mass is taken from the composition.
    pub fn new(pos: DVec3, vel: DVec3, comp: Composition, kind: BodyKind) -> Self {
        Self {
            pos,
            vel,
            mass: comp.total(),
            comp,
            kind,
            moons: Vec::new(),
        }
    }

    /// Re-sync `mass` to the composition total (call after mutating `comp`).
    pub fn sync_mass(&mut self) {
        self.mass = self.comp.total();
    }

    /// Physical radius (AU) from the differentiated composition's volume.
    pub fn physical_radius(&self) -> f64 {
        let v_cm3 = self.comp.volume_cm3();
        if v_cm3 <= 0.0 {
            return 0.0;
        }
        let r_cm = (3.0 * v_cm3 / (4.0 * std::f64::consts::PI)).cbrt();
        r_cm / AU_CM
    }

    /// Inflated radius used for collision detection (see [`COLLISION_INFLATION`]).
    pub fn collision_radius(&self) -> f64 {
        self.physical_radius() * COLLISION_INFLATION
    }

    /// Distance from the star (AU).
    pub fn helio_radius(&self) -> f64 {
        self.pos.length()
    }

    /// Whether the body is gravitationally bound to the star (elliptical orbit).
    pub fn is_bound(&self) -> bool {
        let r = self.pos.length();
        if r <= 0.0 {
            return false;
        }
        let mu = G * (M_STAR + self.mass);
        0.5 * self.vel.length_squared() - mu / r < 0.0
    }

    /// Orbital semi-major axis (AU) and eccentricity from the heliocentric state
    /// vector. For an unbound (hyperbolic) orbit `a` is negative and `e ≥ 1`.
    pub fn orbital_elements(&self) -> (f64, f64) {
        let mu = G * (M_STAR + self.mass);
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

    /// Orbital period (yr) for a bound orbit; `None` if unbound.
    pub fn period(&self) -> Option<f64> {
        let (a, _) = self.orbital_elements();
        if a.is_finite() && a > 0.0 {
            let mu = G * (M_STAR + self.mass);
            Some(std::f64::consts::TAU * (a * a * a / mu).sqrt())
        } else {
            None
        }
    }

    /// Whether this body counts as a giant (captured a gas envelope).
    pub fn is_giant(&self) -> bool {
        self.kind == BodyKind::Giant
    }

    /// The body's dominant material class (its "type"), for colour/label.
    pub fn dominant(&self) -> Option<MaterialClass> {
        self.comp.dominant()
    }
}

/// Has `bodies[idx]` **cleared its orbital neighbourhood** — the IAU criterion that
/// separates a *planet* from a *dwarf*? True if the body outweighs the sum of all other
/// (non-giant) bodies sharing its orbital band (within ±`BAND_FRACTION` of its semi-major
/// axis). Earth dominates its lane → planet; a body lost in a crowded belt does not → dwarf.
/// Used only for classification/labelling, never for the dynamics.
pub fn cleared_neighborhood(idx: usize, bodies: &[Body]) -> bool {
    /// Neighbourhood half-width as a fraction of the body's semi-major axis.
    const BAND_FRACTION: f64 = 0.15;
    let body = &bodies[idx];
    if body.kind == BodyKind::Giant {
        return true; // giants own their region by definition here
    }
    let (a, _) = body.orbital_elements();
    if a <= 0.0 || a.is_nan() {
        return false;
    }
    let band = BAND_FRACTION * a;
    let mut neighbour_mass = 0.0;
    for (j, o) in bodies.iter().enumerate() {
        if j == idx || o.kind == BodyKind::Giant {
            continue;
        }
        let (ao, _) = o.orbital_elements();
        if ao > 0.0 && (ao - a).abs() < band {
            neighbour_mass += o.mass;
        }
    }
    body.mass > neighbour_mass
}

/// A circular-orbit velocity (AU/yr) at radius `r` for the given body mass, in the
/// disk plane and perpendicular to `dir_in_plane`'s radial — used to seed embryos.
pub fn circular_speed(r: f64, body_mass: f64) -> f64 {
    if r <= 0.0 {
        return 0.0;
    }
    (G * (M_STAR + body_mass) / r).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::astro::M_EARTH;

    fn earthlike(a_au: f64) -> Body {
        // A roughly Earth-mass rock+metal body on a circular orbit at `a_au`.
        let mut comp = Composition::of(MaterialClass::Silicate, 0.68 * M_EARTH);
        comp.add(&Composition::of(MaterialClass::Metal, 0.32 * M_EARTH));
        let pos = DVec3::new(a_au, 0.0, 0.0);
        let v = circular_speed(a_au, comp.total());
        let vel = DVec3::new(0.0, 0.0, v);
        Body::new(pos, vel, comp, BodyKind::Protoplanet)
    }

    #[test]
    fn circular_orbit_recovers_its_radius_and_period() {
        let b = earthlike(1.0);
        let (a, e) = b.orbital_elements();
        assert!((a - 1.0).abs() < 1e-6, "a ≈ 1 AU, got {a}");
        assert!(e < 1e-6, "near-circular, e = {e}");
        // 1 AU around 1 M☉ is ~1 year.
        let p = b.period().unwrap();
        assert!((p - 1.0).abs() < 1e-3, "period ≈ 1 yr, got {p}");
        assert!(b.is_bound());
    }

    #[test]
    fn earth_radius_is_about_right() {
        let b = earthlike(1.0);
        // Earth's radius is ~4.26e-5 AU; differentiated rock+iron lands close.
        let r = b.physical_radius();
        assert!(r > 3.0e-5 && r < 6.0e-5, "earthlike radius {r} AU off");
    }

    #[test]
    fn unbound_body_reads_as_unbound() {
        let mut b = earthlike(1.0);
        b.vel *= 2.0; // well above escape
        assert!(!b.is_bound());
        let (a, e) = b.orbital_elements();
        assert!(a < 0.0, "hyperbolic a negative, got {a}");
        assert!(e >= 1.0, "hyperbolic e ≥ 1, got {e}");
    }

    #[test]
    fn clearing_separates_planets_from_a_crowded_belt() {
        // One Earth alone at 1 AU clears its lane → planet.
        let lone = vec![earthlike(1.0)];
        assert!(cleared_neighborhood(0, &lone));

        // Three equal-mass bodies crammed into the same band: none outweighs the others,
        // so none has "cleared" it — all dwarfs (an un-consolidated belt).
        let crowd = vec![earthlike(0.98), earthlike(1.0), earthlike(1.02)];
        for i in 0..3 {
            assert!(!cleared_neighborhood(i, &crowd), "body {i} shares its band → dwarf");
        }
    }
}
