//! **flicker-orrery** — the fixed Prism planet-layout model (an *orrery*: a model
//! of the worlds and their orbits).
//!
//! GPU-free and **shared**: both the solar-birth cinematic and the from-Home
//! heliocentric sky read the SAME roster from here — one source of truth, never
//! copied. It is *always* the same eight planets in the ruled order (inner →
//! outer), the sun at the origin, and Home's moon.
//!
//! **What is Prism-ruled** (BookV cosmology): the roster, the inner→outer order,
//! the one-planet-per-school mapping and each school's colour, rings on Air, Death
//! being occulted (known by shadow-transit), and the *composition classes* the book
//! spells out — Light is "the white **gas giant**", Air "the ringed planet", Water
//! "**Neptune** blue" (an ice giant), Death the outermost, occulted dwarf. **What is
//! a rendering choice** (tune freely): the orbit *elements* (a/e/i/Ω) and the visual
//! *sizes*.
//!
//! **On "equal apparent sizes":** BookV rules the seven share one *apparent* size —
//! about a quarter of the sun/moon disc **as seen looking up from Home's sky**. That
//! governs the from-Home sky view, NOT the god's-eye cinematic, where bodies are
//! sized by their composition class for a grounded, legible look (gas giants
//! largest, dwarf tiny). No canon is bent — a different view, a different rule.
//!
//! **Reckoning:** orbital periods follow Kepler's third law (T ∝ a^1.5) and are
//! anchored so **Home's period is one canonical year** ([`orbital_period_years`]),
//! giving a calendar / celestial panel one shared clock to read. (The *moon* is a
//! separate, canon-real-ephemeris matter — Book V — not modelled here.)

use glam::Vec3;

/// Inner / outer radius of the system in the viewer's AU-like layout units. `OUTER`
/// clears the outermost body's aphelion (the eccentric dwarf, Death) with margin, so
/// a dust cloud or orbit rings frame the whole system. A **layout** scale, not a
/// Prism-ruled distance.
pub const SYSTEM_INNER: f32 = 0.4;
pub const SYSTEM_OUTER: f32 = 15.5;

/// Home's semi-major axis — the anchor for the one-year period (see
/// [`orbital_period_years`]). Kept as a const so the period math needn't dig it back
/// out of the roster; the roster uses it for Home's `a`.
pub const A_HOME: f32 = 2.8;

/// Cinematic seconds for Home to complete one orbit — i.e. one canonical Prism
/// **year**. The whole roster's angular rates hang off this single anchor, so a
/// calendar / celestial panel reads one clock. Purely a cosmetic pace; retune freely
/// without disturbing the reckoning (the *ratios* between bodies are fixed by
/// Kepler's third law, not by this number).
pub const HOME_YEAR_SECONDS: f32 = 160.0;

/// Composition class of a body (BookV). Drives the visual size (and could drive
/// appearance): rocky worlds small, gas giants largest, the ice giant mid, the dwarf
/// tiny.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BodyKind {
    Rocky,
    GasGiant,
    IceGiant,
    Dwarf,
}

impl BodyKind {
    /// Short HUD label for this class.
    pub fn label(self) -> &'static str {
        match self {
            BodyKind::Rocky => "rocky",
            BodyKind::GasGiant => "gas giant",
            BodyKind::IceGiant => "ice giant",
            BodyKind::Dwarf => "dwarf",
        }
    }
}

/// A member of the fixed roster. The orbit is a tilted Keplerian ellipse with the
/// sun at a focus (the origin): semi-major axis `a`, eccentricity `e`, inclination
/// `incl`, longitude of ascending node `node` (Ω); `phase0` is the starting angle.
pub struct Planet {
    pub name: &'static str,
    /// Composition class (BookV) — sets the visual size.
    pub kind: BodyKind,
    /// School colour (Prism-ruled), used unlit — the sun's point light shades it.
    pub color: [f32; 3],
    /// Semi-major axis (layout units — a rendering choice, not Prism-ruled).
    pub a: f32,
    /// Orbital eccentricity (0 = circle). Small for the inner rockies; largest for
    /// the outer dwarf (a grounded, Pluto-like swing).
    pub e: f32,
    /// Orbital inclination (radians) — tilts the orbit plane out of the reference
    /// (XZ) plane. A few degrees for most; steeper for the dwarf.
    pub incl: f32,
    /// Longitude of the ascending node (radians, Ω) — the compass direction the tilt
    /// leans, spread per body so the planes don't all hinge the same way.
    pub node: f32,
    /// Visual sphere radius (layout units), set by composition class.
    pub radius: f32,
    /// Starting orbital angle (radians), spread so the planets don't line up.
    pub phase0: f32,
    /// Air alone wears rings.
    pub rings: bool,
    /// Death is occulted — rendered near-black, known only by its shadow-transit.
    pub occulted: bool,
    /// Home alone carries the moon.
    pub moon: bool,
}

/// The canonical roster, inner → outer: Chaos · Fire · **Home** · Earth · Light ·
/// **Air** (rings) · Water · **Death** (occulted). Home carries the moon.
///
/// Composition classes follow BookV (four inner rockies, two gas giants, the ice
/// giant Water, the dwarf Death) and set the visual sizes — gas giants largest,
/// dwarf tiny. Distances echo a real system's *shape* (a layout choice, not a ruled
/// distance): a tight inner cluster, a snow-line gap between Earth and Light, widely
/// spaced giants, a far eccentric dwarf (the r^-3/2 / ~2.7-AU snow-line reasoning of
/// the salvaged formation model). Each body carries its own small eccentricity and
/// inclination, so the orbits are tilted ellipses, not coplanar circles — all fixed
/// per-body constants (varying by index/class), never random.
pub fn roster() -> Vec<Planet> {
    use BodyKind::{Dwarf, GasGiant, IceGiant, Rocky};
    // One roster row, factored into a named type so the array stays legible.
    // (name, class, school colour, a, e, inclination°, radius, rings, occulted, moon)
    //
    // Sizes are grounded-but-legible: rockies ~0.26–0.32, ice giant 0.42, gas giants
    // 0.54–0.60 (Light>Air, like Jupiter>Saturn), dwarf 0.15 (smallest of all).
    // Eccentricity/inclination are subtle for the inner worlds and modest-but-visible
    // for Death.
    type Def = (
        &'static str,
        BodyKind,
        [f32; 3],
        f32,
        f32,
        f32,
        f32,
        bool,
        bool,
        bool,
    );
    #[rustfmt::skip]
    let defs: [Def; 8] = [
        ("Chaos", Rocky,    [0.95, 0.45, 0.10], 1.4,    0.08,  6.0, 0.26, false, false, false), // orange
        ("Fire",  Rocky,    [0.86, 0.16, 0.11], 2.1,    0.03,  3.0, 0.28, false, false, false), // red
        ("Home",  Rocky,    [0.20, 0.52, 0.55], A_HOME, 0.02,  0.6, 0.32, false, false, true),  // habitable (placeholder)
        ("Earth", Rocky,    [0.24, 0.64, 0.26], 3.5,    0.05,  2.0, 0.30, false, false, false), // green
        ("Light", GasGiant, [0.95, 0.96, 0.98], 5.6,    0.05,  1.3, 0.60, false, false, false), // white gas giant
        ("Air",   GasGiant, [0.93, 0.83, 0.20], 8.0,    0.05,  2.5, 0.54, true,  false, false), // yellow, rings
        ("Water", IceGiant, [0.18, 0.40, 0.90], 10.4,   0.03,  1.8, 0.42, false, false, false), // Neptune blue
        ("Death", Dwarf,    [0.05, 0.05, 0.08], 13.0,   0.16, 10.0, 0.15, false, true,  false), // black, occulted
    ];
    defs.into_iter()
        .enumerate()
        .map(
            |(i, (name, kind, color, a, e, incl_deg, radius, rings, occulted, moon))| Planet {
                name,
                kind,
                color,
                a,
                e,
                incl: incl_deg.to_radians(),
                // Golden-angle start, plus a decorrelated node spread, so no two
                // orbits align and the planes hinge in different directions.
                node: (i as f32 * 97.0 + 20.0).to_radians(),
                radius,
                phase0: i as f32 * 2.399_963, // golden angle
                rings,
                occulted,
                moon,
            },
        )
        .collect()
}

/// One canonical Prism **year** in orbital periods: Kepler's third law with Home as
/// the unit (Home's period ≡ 1.0). `a` is the semi-major axis in layout units.
pub fn orbital_period_years(a: f32) -> f32 {
    (a / A_HOME).powf(1.5)
}

/// Mean angular speed (rad/s) for a body of semi-major axis `a`, so Home completes
/// one orbit — one year — every [`HOME_YEAR_SECONDS`]. Inner bodies sweep faster and
/// the far dwarf barely crawls (Kepler III), but the angle advances *uniformly*: the
/// cinematic does not solve Kepler's equation, it lets the ellipse *shape* carry the
/// realism.
pub fn orbit_omega(a: f32) -> f32 {
    std::f32::consts::TAU / (orbital_period_years(a) * HOME_YEAR_SECONDS)
}

/// A point on a body's tilted orbital ellipse at angle `theta` (radians, measured
/// from perihelion). The ellipse has the sun at a focus (the origin): polar radius
/// `r = a(1−e²)/(1 + e·cosθ)`. The flat ellipse (in the XZ reference plane) is then
/// tilted by the inclination `incl` about its ascending-node line, whose compass
/// direction is `node` (Ω).
fn ellipse_point(p: &Planet, theta: f32) -> Vec3 {
    let (st, ct) = theta.sin_cos();
    let r = p.a * (1.0 - p.e * p.e) / (1.0 + p.e * ct);
    // Flat ellipse in the reference (XZ) plane, perihelion toward local +X.
    let (lx, lz) = (r * ct, r * st);
    // Tilt the plane by `incl` about the X axis, then swing the node to Ω about the
    // vertical (Y) axis: pos = R_y(Ω) · R_x(incl) · (lx, 0, lz).
    let (si, ci) = p.incl.sin_cos();
    let (sn, cn) = p.node.sin_cos();
    let (ty, tz) = (-lz * si, lz * ci); // after R_x(incl): (lx, ty, tz)
    Vec3::new(lx * cn + tz * sn, ty, -lx * sn + tz * cn)
}

/// A planet's world position at animation time `t` (seconds): its angle advances
/// uniformly from `phase0` at [`orbit_omega`], tracing the tilted ellipse.
pub fn planet_pos(p: &Planet, t: f32) -> Vec3 {
    ellipse_point(p, p.phase0 + t * orbit_omega(p.a))
}

/// A body's orbit as a closed loop of `segs` line segments — the faint reference
/// ring. It samples the *same* tilted ellipse [`planet_pos`] travels, so the ring and
/// the body can never drift apart (one source of truth for the orbit geometry).
pub fn orbit_ellipse(p: &Planet, segs: usize) -> Vec<(Vec3, Vec3)> {
    use std::f32::consts::TAU;
    let pt = |i: usize| ellipse_point(p, i as f32 / segs as f32 * TAU);
    (0..segs).map(|i| (pt(i), pt(i + 1))).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roster_is_the_ruled_eight() {
        use BodyKind::{Dwarf, GasGiant, IceGiant, Rocky};
        let r = roster();
        let names: Vec<&str> = r.iter().map(|p| p.name).collect();
        assert_eq!(
            names,
            ["Chaos", "Fire", "Home", "Earth", "Light", "Air", "Water", "Death"]
        );
        // Semi-major axes strictly increase inner → outer.
        assert!(r.windows(2).all(|w| w[1].a > w[0].a));
        // Exactly one moon-bearer (Home), one ringed (Air), one occulted (Death).
        assert_eq!(r.iter().filter(|p| p.moon).count(), 1);
        assert_eq!(r.iter().filter(|p| p.rings).count(), 1);
        assert_eq!(r.iter().filter(|p| p.occulted).count(), 1);
        assert_eq!(r.iter().find(|p| p.moon).unwrap().name, "Home");
        assert_eq!(r.iter().find(|p| p.rings).unwrap().name, "Air");
        assert_eq!(r.iter().find(|p| p.occulted).unwrap().name, "Death");

        // Composition classes follow BookV: four inner rockies, two gas giants, the
        // ice giant Water, the dwarf Death.
        let kinds: Vec<BodyKind> = r.iter().map(|p| p.kind).collect();
        assert_eq!(
            kinds,
            [Rocky, Rocky, Rocky, Rocky, GasGiant, GasGiant, IceGiant, Dwarf]
        );

        // Sizes are driven by class, NOT equal (the deliberate override of BookV's
        // "equal apparent sizes", which is a from-Home *sky* rule — see module docs):
        // gas giants largest, then the ice giant, then the rockies, dwarf smallest.
        let smallest_giant = r
            .iter()
            .filter(|p| p.kind == GasGiant)
            .map(|p| p.radius)
            .fold(f32::MAX, f32::min);
        let ice = r.iter().find(|p| p.kind == IceGiant).unwrap().radius;
        let biggest_rocky = r
            .iter()
            .filter(|p| p.kind == Rocky)
            .map(|p| p.radius)
            .fold(0.0_f32, f32::max);
        let dwarf = r.iter().find(|p| p.kind == Dwarf).unwrap().radius;
        assert!(smallest_giant > ice, "gas giants larger than the ice giant");
        assert!(ice > biggest_rocky, "ice giant larger than any rocky");
        assert!(biggest_rocky > dwarf, "every rocky larger than the dwarf");
        assert!(
            r.iter().all(|p| p.radius >= dwarf),
            "the dwarf (Death) is the smallest body"
        );

        // Orbits are eccentric ellipses: inner rockies nearly circular; the dwarf the
        // most eccentric of all (a grounded, Pluto-like swing).
        assert!(
            r.iter().filter(|p| p.kind == Rocky).all(|p| p.e < 0.1),
            "inner rockies are near-circular"
        );
        let death = r.iter().find(|p| p.name == "Death").unwrap();
        assert!(
            r.iter().all(|p| p.name == "Death" || p.e < death.e),
            "Death is the most eccentric body"
        );
        // And it clears the layout envelope at aphelion.
        assert!(
            death.a * (1.0 + death.e) < SYSTEM_OUTER,
            "Death's aphelion stays inside SYSTEM_OUTER"
        );

        // The reckoning anchor: Home's orbital period is exactly one canonical year,
        // and periods grow with distance (Kepler III), so Death's year is longest.
        let home = r.iter().find(|p| p.name == "Home").unwrap();
        assert!(
            (orbital_period_years(home.a) - 1.0).abs() < 1e-6,
            "Home = one year"
        );
        assert!(orbital_period_years(death.a) > orbital_period_years(home.a));
    }

    #[test]
    fn planet_pos_respects_orbit_geometry() {
        // A planet stays on its ellipse: at every sampled angle the distance to the
        // sun (origin) lies within [a(1−e), a(1+e)], and both extremes are reached.
        let r = roster();
        let p = r.iter().find(|p| p.name == "Death").unwrap();
        let (peri, apo) = (p.a * (1.0 - p.e), p.a * (1.0 + p.e));
        let mut min_d = f32::MAX;
        let mut max_d = 0.0_f32;
        for k in 0..360 {
            let d = ellipse_point(p, (k as f32).to_radians()).length();
            assert!(d >= peri - 1e-3 && d <= apo + 1e-3, "on the ellipse");
            min_d = min_d.min(d);
            max_d = max_d.max(d);
        }
        assert!((min_d - peri).abs() < 1e-2, "reaches perihelion");
        assert!((max_d - apo).abs() < 1e-2, "reaches aphelion");
        // An inclined orbit leaves the reference plane (non-zero Y somewhere).
        assert!(
            (0..360).any(|k| ellipse_point(p, (k as f32).to_radians()).y.abs() > 1e-2),
            "an inclined orbit rises out of the XZ plane"
        );
    }
}
