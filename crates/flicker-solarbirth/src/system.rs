//! The **fixed Prism system** — the canonical roster, plus the small mesh helpers
//! (UV sphere + ring annulus) used to draw it.
//!
//! This replaces the old emergent generator entirely: no random seed, no N-body,
//! no material ledger. The system is *always* the same eight planets in the ruled
//! order (inner → outer), the sun at the origin, and Home's moon.
//!
//! **What is Prism-ruled** (the celestial design sessions): the roster, the
//! inner→outer order, the one-planet-per-school mapping and each school's colour,
//! rings on Air, and Death being occulted (known by shadow-transit). **What is a
//! rendering choice here** (NOT Prism data — tune freely): the orbit *radii* and
//! visual *sizes* (Prism rules order and "equal apparent sizes", not distances),
//! and Home's placeholder colour (its school colour was never ruled).

use flicker::render::{MeshVertex, Vec3};

/// Inner / outer radius of the system in the viewer's AU-like layout units. The
/// dust cloud spans a little past the outermost planet (Death). A **layout**
/// scale, not Prism-ruled distance.
pub const SYSTEM_INNER: f32 = 0.4;
pub const SYSTEM_OUTER: f32 = 15.0;

/// A member of the fixed roster.
pub struct Planet {
    pub name: &'static str,
    /// School colour (Prism-ruled), used unlit — the sun's point light shades it.
    pub color: [f32; 3],
    /// Orbit radius (layout units — a rendering choice, not Prism-ruled).
    pub orbit: f32,
    /// Visual sphere radius (layout units — a rendering choice).
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
/// Orbit radii ramp outward with widening gaps (a Bode-like *visual* spacing —
/// not a ruled distance). Visual sizes are near-equal, nodding to Prism's "equal
/// apparent sizes" (Air a touch larger so its rings read).
pub fn roster() -> Vec<Planet> {
    // `phase0` (the golden angle × slot) is filled in below so no two planets
    // start aligned; the other fields are the per-planet definition.
    let mk = |name, color, orbit, radius, rings, occulted, moon| Planet {
        name,
        color,
        orbit,
        radius,
        phase0: 0.0,
        rings,
        occulted,
        moon,
    };
    let mut planets = vec![
        mk("Chaos", [0.95, 0.45, 0.10], 1.4, 0.34, false, false, false), // orange
        mk("Fire", [0.86, 0.16, 0.11], 2.5, 0.34, false, false, false),  // red
        mk("Home", [0.20, 0.52, 0.55], 3.9, 0.36, false, false, true),   // habitable (placeholder)
        mk("Earth", [0.24, 0.64, 0.26], 5.4, 0.34, false, false, false), // green
        mk("Light", [0.95, 0.96, 0.98], 7.0, 0.34, false, false, false), // white
        mk("Air", [0.93, 0.83, 0.20], 8.9, 0.40, true, false, false),    // yellow, rings
        mk("Water", [0.18, 0.40, 0.90], 11.0, 0.34, false, false, false), // blue
        mk("Death", [0.05, 0.05, 0.08], 13.6, 0.34, false, true, false), // black, occulted
    ];
    for (i, p) in planets.iter_mut().enumerate() {
        p.phase0 = i as f32 * 2.39996; // golden angle
    }
    planets
}

/// Angular speed (rad/s) of a planet at layout radius `r`. A cosmetic Kepler-like
/// differential (`ω ∝ r^-3/2`) so inner planets sweep faster — no physics, no
/// accounting, just a living-system look.
pub fn orbit_omega(r: f32) -> f32 {
    const ORBIT_SPEED: f32 = 0.20;
    ORBIT_SPEED / r.powf(1.5)
}

/// A planet's world position at animation time `t` (seconds): a circular orbit in
/// the disk plane (XZ), starting from `phase0`.
pub fn planet_pos(p: &Planet, t: f32) -> Vec3 {
    let a = p.phase0 + t * orbit_omega(p.orbit);
    Vec3::new(p.orbit * a.cos(), 0.0, p.orbit * a.sin())
}

/// Pack an RGB colour into the mesh shader's direct-RGB666 escape: low 12 bits =
/// `0xFFF`, then 6-bit channels in bits 12-17 (R) / 18-23 (G) / 24-29 (B). Lets a
/// mesh carry a literal colour instead of a material-table index.
pub fn pack_rgb(c: [f32; 3]) -> u32 {
    let q = |x: f32| (((x.clamp(0.0, 1.0) * 63.0) + 0.5) as u32) & 0x3F;
    0xFFFu32 | (q(c[0]) << 12) | (q(c[1]) << 18) | (q(c[2]) << 24)
}

/// A unit UV sphere, every vertex carrying the flat surface colour `color` (unlit;
/// the sun point light shades it) and an outward normal. Wound CCW-outward to match
/// the mesh pipeline's back-face cull (`FrontFace::Ccw`, `cull Back`).
pub fn uv_sphere(color: [f32; 3], sectors: u32, stacks: u32) -> (Vec<MeshVertex>, Vec<u32>) {
    use std::f32::consts::PI;
    let mat = pack_rgb(color);
    let stride = sectors + 1;
    let mut verts = Vec::with_capacity(((stacks + 1) * stride) as usize);
    for i in 0..=stacks {
        let phi = PI * i as f32 / stacks as f32; // 0 (north pole) → PI (south)
        let (sp, cp) = phi.sin_cos();
        for j in 0..=sectors {
            let theta = std::f32::consts::TAU * j as f32 / sectors as f32;
            let (st, ct) = theta.sin_cos();
            let n = [sp * ct, cp, sp * st];
            verts.push(MeshVertex { position: n, normal: n, material: mat });
        }
    }
    let mut idx = Vec::with_capacity((stacks * sectors * 6) as usize);
    for i in 0..stacks {
        for j in 0..sectors {
            let a = i * stride + j;
            let b = a + stride;
            // CCW-outward (verified against the mesh pipeline's back-face cull).
            idx.extend_from_slice(&[a, a + 1, b, a + 1, b + 1, b]);
        }
    }
    (verts, idx)
}

/// A flat banded ring annulus in the local XZ plane (`y = 0`), radii in
/// `[inner, outer]` (units of the planet radius). Concentric greyscale brightness
/// bands (a Cassini-division feel); the caller tilts, scales, and **tints** it.
/// Double-sided so it shows from either face. Salvaged from the old `planet.rs`.
pub fn ring_mesh(inner: f32, outer: f32, segments: usize, bands: usize) -> (Vec<MeshVertex>, Vec<u32>) {
    use std::f32::consts::TAU;
    let stride = segments + 1;
    let mut verts = Vec::with_capacity((bands + 1) * stride);
    for bi in 0..=bands {
        let r = inner + (outer - inner) * bi as f32 / bands as f32;
        let b = 0.45 + 0.55 * (0.5 + 0.5 * (bi as f32 * 2.7).sin()); // concentric bands / gaps
        let m = pack_rgb([b, b, b]);
        for si in 0..=segments {
            let a = si as f32 / segments as f32 * TAU;
            let (s, c) = a.sin_cos();
            verts.push(MeshVertex { position: [r * c, 0.0, r * s], normal: [0.0, 1.0, 0.0], material: m });
        }
    }
    let mut idx = Vec::with_capacity(bands * segments * 12);
    for bi in 0..bands {
        for si in 0..segments {
            let a = (bi * stride + si) as u32;
            let b = a + stride as u32;
            idx.extend_from_slice(&[a, b, a + 1, a + 1, b, b + 1]); // front
            idx.extend_from_slice(&[a + 1, b, a, b + 1, b, a + 1]); // back (double-sided)
        }
    }
    (verts, idx)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roster_is_the_ruled_eight() {
        let r = roster();
        let names: Vec<&str> = r.iter().map(|p| p.name).collect();
        assert_eq!(names, ["Chaos", "Fire", "Home", "Earth", "Light", "Air", "Water", "Death"]);
        // Orbits strictly increase inner → outer.
        assert!(r.windows(2).all(|w| w[1].orbit > w[0].orbit));
        // Exactly one moon-bearer (Home), one ringed (Air), one occulted (Death).
        assert_eq!(r.iter().filter(|p| p.moon).count(), 1);
        assert_eq!(r.iter().filter(|p| p.rings).count(), 1);
        assert_eq!(r.iter().filter(|p| p.occulted).count(), 1);
        assert!(r.iter().find(|p| p.moon).unwrap().name == "Home");
        assert!(r.iter().find(|p| p.rings).unwrap().name == "Air");
    }

    #[test]
    fn direct_rgb_escape_round_trips() {
        let m = pack_rgb([1.0, 0.0, 0.5]);
        assert_eq!(m & 0xFFF, 0xFFF, "direct-RGB escape marker set");
        assert_eq!((m >> 12) & 0x3F, 63, "R = full");
        assert_eq!((m >> 18) & 0x3F, 0, "G = none");
        assert_eq!((m >> 24) & 0x3F, 32, "B ≈ half");
    }
}
