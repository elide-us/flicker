//! Slice 3b — the **Snyder equal-area icosahedral projection** (ISEA).
//!
//! This is the placement rule for subdivision lattice sites. It replaces the
//! cheap "barycentric then normalise" projection, which is gnomonic-ish and
//! inflates cells toward the middle of a face: at freq 8 the hex areas spread
//! ~1.8×, and the imprint GROWS with resolution, so anything the simulation
//! derives from area (thickness, flux, submerged fraction) inherits it and the
//! icosahedron's own seams print through into the world.
//!
//! Equal area is load-bearing for two reasons beyond tidiness. The ledgers store
//! **absolute** element masses, so equal amounts only mean equal concentration
//! when cells are equal-area. And the tectonic conveyor moves a column **one hex
//! step** at a time, which is only a coherent notion of speed if a step is the
//! same distance everywhere.
//!
//! # How it works
//!
//! Each icosahedron face is a spherical equilateral triangle; the lattice is
//! defined on a planar equilateral triangle. Snyder's map between them splits the
//! face into **six congruent sub-triangles** (centre → vertex → edge midpoint) and
//! is area-true inside each. Two rules do all the work:
//!
//! 1. **Azimuth.** Sweeping from the vertex ray toward the edge-midpoint ray, the
//!    fraction of *planar* area swept must equal the fraction of *spherical* area
//!    swept. The planar side is a closed form; the spherical side is a spherical
//!    excess, which has no closed inverse, so [`sphere_azimuth`] bisects it.
//! 2. **Radius.** Along a matched pair of azimuths, area-trueness forces
//!    `ρ² ∝ (1 − cos z)`, and the constant is pinned by the boundary (the planar
//!    edge maps to the spherical edge). So `(ρ/ρ_max)² = (1 − cos z)/(1 − cos q)`
//!    — no derivative needed.
//!
//! Adjacency, shards and cell ids are untouched: this moves points only.
//!
//! # Why shared edges get their own path
//!
//! A lattice site **on a face edge belongs to two faces**, and the two faces have
//! different centres — so running the general rule twice would give two positions
//! differing in the last bits, and [`crate::mesh::Weld`] would fuse them only by
//! luck. [`edge_point`] therefore places edge sites from the edge alone (canonical
//! endpoint order + constants of the icosahedron, never a face centre), so both
//! faces compute **bit-identical** positions and the weld is exact.

use glam::{DVec3, Vec3};

/// Half the spherical face's interior angle at a vertex: five faces meet at an
/// icosahedron vertex, so the full angle is 72° and this is 36°.
const G: f64 = std::f64::consts::PI / 5.0;

/// Half the planar equilateral triangle's interior angle (60° / 2).
const THETA: f64 = std::f64::consts::PI / 6.0;

/// The azimuth one sub-triangle spans about the face centre. The three vertices
/// sit 120° apart around the centre and the edge-midpoint ray bisects that, so
/// each of the six sub-triangles spans 60°.
const SUB: f64 = std::f64::consts::PI / 3.0;

/// Area of one sub-triangle on the unit sphere: 4π over 20 faces × 6 = π/30
/// steradians (a 6° spherical excess).
const SUB_AREA: f64 = std::f64::consts::PI / 30.0;

/// Angular length of an icosahedron edge, `acos(1/√5)` ≈ 63.4349°.
fn edge_angle() -> f64 {
    (1.0 / 5f64.sqrt()).acos()
}

/// Spherical distance from a face centre to one of its vertices, ≈ 37.3774°.
///
/// Derived from the icosahedron's edge angle rather than typed in as a magic
/// number — and deliberately a **constant**, not a per-face measurement, because
/// [`edge_point`] must produce the same bits from either of the two faces that
/// share an edge.
fn face_circumradius() -> f64 {
    let c = 1.0 / 5f64.sqrt(); // cosine of the edge angle
    ((1.0 + 2.0 * c) / (3.0 + 6.0 * c).sqrt()).acos()
}

/// The third angle of the spherical triangle (centre, vertex, edge point) whose
/// angle at the centre is `az`: two angles and the side between them are known
/// (`G` at the vertex, `g` from centre to vertex), so the third follows.
fn third_angle(az: f64, g: f64) -> f64 {
    (-az.cos() * G.cos() + az.sin() * G.sin() * g.cos())
        .clamp(-1.0, 1.0)
        .acos()
}

/// Spherical area swept from the vertex ray out to azimuth `az` — the excess of
/// that triangle. Zero at `az = 0`, [`SUB_AREA`] at 60°, monotone between.
fn swept_area(az: f64, g: f64) -> f64 {
    az + G + third_angle(az, g) - std::f64::consts::PI
}

/// The sphere azimuth that sweeps the same *fraction* of its sub-triangle as the
/// planar azimuth `az_p` (both measured from the vertex ray, 0..60°).
///
/// The planar wedge area is `½·sin(az)·sin θ / sin(θ + az)`, so the fraction is a
/// closed form; the spherical side is [`swept_area`], which has no closed inverse
/// and is bisected. It is monotone, so this always converges.
fn sphere_azimuth(az_p: f64, g: f64) -> f64 {
    let fraction = az_p.sin() / (THETA + az_p).sin() / SUB.sin();
    let target = fraction * SUB_AREA;
    let (mut lo, mut hi) = (0.0f64, SUB);
    for _ in 0..40 {
        let mid = 0.5 * (lo + hi);
        if swept_area(mid, g) < target {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    0.5 * (lo + hi)
}

/// Distance from the face centre to the face edge along sphere azimuth `az`.
fn centre_to_edge(az: f64, g: f64) -> f64 {
    let sin_q = G.sin() * g.sin() / third_angle(az, g).sin();
    sin_q.clamp(-1.0, 1.0).asin()
}

/// Wrap an angle into `(-π, π]`.
fn wrap_pi(a: f64) -> f64 {
    let tau = std::f64::consts::TAU;
    let mut x = a % tau;
    if x > std::f64::consts::PI {
        x -= tau;
    } else if x <= -std::f64::consts::PI {
        x += tau;
    }
    x
}

/// One face's precomputed frame — built once per face, then reused for every
/// lattice site on it.
pub(crate) struct FaceFrame {
    /// The face's spherical centroid.
    centre: DVec3,
    /// Tangent at the centre pointing at vertex `s`.
    to_vertex: [DVec3; 3],
    /// Tangent at the centre perpendicular to `to_vertex[s]`, on the side of
    /// vertex `s + 1` — so a positive azimuth sweeps the same way the planar
    /// frame's positive angle does, whatever the face's winding.
    across: [DVec3; 3],
    /// Spherical distance from the centre to a vertex (measured, not assumed —
    /// [`face_circumradius`] is the analytic twin and the two are tested equal).
    g: f64,
}

impl FaceFrame {
    pub fn new(a: Vec3, b: Vec3, c: Vec3) -> Self {
        let v = [
            a.as_dvec3().normalize(),
            b.as_dvec3().normalize(),
            c.as_dvec3().normalize(),
        ];
        let centre = (v[0] + v[1] + v[2]).normalize();
        let tangent = |t: DVec3| (t - centre * centre.dot(t)).normalize();
        let to_vertex = [tangent(v[0]), tangent(v[1]), tangent(v[2])];
        let across = std::array::from_fn(|s: usize| {
            let next = to_vertex[(s + 1) % 3];
            (next - to_vertex[s] * to_vertex[s].dot(next)).normalize()
        });
        let g = centre.dot(v[0]).clamp(-1.0, 1.0).acos();
        Self {
            centre,
            to_vertex,
            across,
            g,
        }
    }
}

/// Place the lattice site with barycentric weights `(i, j, k)` (summing to `m`)
/// **inside** a face. Callers must route sites on an edge through [`edge_point`]
/// and corners straight to the vertex — see the module note on welding.
pub(crate) fn lattice_point(frame: &FaceFrame, i: u32, j: u32, k: u32, m: u32) -> Vec3 {
    // The canonical planar equilateral triangle: circumradius 1, vertex `s` at
    // azimuth 120°·s. Barycentric weights are affine, so this stands in for the
    // face exactly and needs no knowledge of the face's own planar embedding.
    let mf = m as f64;
    let w = [i as f64 / mf, j as f64 / mf, k as f64 / mf];
    let (mut x, mut y) = (0.0f64, 0.0f64);
    for (s, &ws) in w.iter().enumerate() {
        let ang = std::f64::consts::TAU * s as f64 / 3.0;
        x += ws * ang.cos();
        y += ws * ang.sin();
    }
    let rho = (x * x + y * y).sqrt();
    if rho < 1e-12 {
        return frame.centre.as_vec3(); // the face centre maps to itself
    }

    // Which of the six sub-triangles: the nearest vertex ray, and which side.
    let alpha = y.atan2(x);
    let (s, delta) = (0..3usize)
        .map(|s| (s, wrap_pi(alpha - std::f64::consts::TAU * s as f64 / 3.0)))
        .min_by(|a, b| {
            a.1.abs()
                .partial_cmp(&b.1.abs())
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .expect("three sub-triangle candidates");
    let az_p = delta.abs().min(SUB);

    let az_s = sphere_azimuth(az_p, frame.g);
    let q = centre_to_edge(az_s, frame.g);
    // Area-trueness along the ray: (ρ/ρ_max)² = (1 − cos z)/(1 − cos q).
    let rho_max = THETA.sin() / (THETA + az_p).sin();
    let f = (rho / rho_max).clamp(0.0, 1.0);
    let z = (1.0 - f * f * (1.0 - q.cos())).clamp(-1.0, 1.0).acos();

    let side = if delta < 0.0 { -1.0 } else { 1.0 };
    let dir = frame.to_vertex[s] * az_s.cos() + frame.across[s] * (az_s.sin() * side);
    (frame.centre * z.cos() + dir * z.sin())
        .normalize()
        .as_vec3()
}

/// Total order on a direction, by bit pattern — used only to pick a canonical
/// endpoint for a shared edge. Both faces pass the same `Vec3` values, so this
/// agrees on both sides; it needs no geometric meaning.
fn endpoint_key(v: Vec3) -> [u32; 3] {
    [v.x.to_bits(), v.y.to_bits(), v.z.to_bits()]
}

/// Arc distance from the `a` end of an edge to the lattice site at fraction `f`.
/// Each half of the edge is placed from its own vertex's sub-triangle, so the
/// midpoint is the seam and the two halves are mirror images.
fn edge_arc(f: f64, g: f64) -> f64 {
    if f > 0.5 {
        return edge_angle() - edge_arc(1.0 - f, g);
    }
    // The planar site on the canonical triangle's vertex-0 → vertex-1 edge, and
    // its azimuth about the centre measured from the vertex-0 ray.
    let (x, y) = (1.0 - 1.5 * f, f * 3f64.sqrt() / 2.0);
    let az_s = sphere_azimuth(y.atan2(x), g);
    // In the (centre, vertex, site) triangle the side we want is opposite the
    // angle at the centre.
    (az_s.sin() * g.sin() / third_angle(az_s, g).sin())
        .clamp(-1.0, 1.0)
        .asin()
}

/// Place the lattice site `n` of `m` along the edge from `a` to `b`.
///
/// Depends only on the edge — never on a face centre — and orders the endpoints
/// canonically, so the two faces sharing this edge produce **bit-identical**
/// positions and [`crate::mesh::Weld`] fuses them exactly. (If this ever drifted,
/// the cell count would stop matching `10·freq² + 2`, which is a standing test.)
pub(crate) fn edge_point(a: Vec3, b: Vec3, n: u32, m: u32) -> Vec3 {
    let (a, b, n) = if endpoint_key(b) < endpoint_key(a) {
        (b, a, m - n)
    } else {
        (a, b, n)
    };
    let (a, b) = (a.as_dvec3().normalize(), b.as_dvec3().normalize());
    let arc = edge_arc(n as f64 / m as f64, face_circumradius());
    let toward_b = (b - a * a.dot(b)).normalize();
    (a * arc.cos() + toward_b * arc.sin())
        .normalize()
        .as_vec3()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh::icosahedron;

    /// The analytic centre-to-vertex distance is what a real face measures — the
    /// edge placement leans on the analytic one, so a drift between them would
    /// silently break welding.
    #[test]
    fn analytic_and_measured_face_circumradius_agree() {
        let (verts, faces) = icosahedron();
        let f = faces[0];
        let frame = FaceFrame::new(verts[f[0]], verts[f[1]], verts[f[2]]);
        assert!(
            (frame.g - face_circumradius()).abs() < 1e-6,
            "measured {} vs analytic {}",
            frame.g,
            face_circumradius()
        );
        // ≈ 37.3774°, the published ISEA value for the icosahedron.
        assert!((face_circumradius().to_degrees() - 37.377_368).abs() < 1e-4);
    }

    /// The two rules meet where they must: the whole sub-triangle sweeps its whole
    /// area, and its azimuths run 0→60° on both sides.
    #[test]
    fn the_sub_triangle_closes() {
        let g = face_circumradius();
        assert!(swept_area(0.0, g).abs() < 1e-12);
        assert!((swept_area(SUB, g) - SUB_AREA).abs() < 1e-12);
        assert!(sphere_azimuth(0.0, g).abs() < 1e-9);
        assert!((sphere_azimuth(SUB, g) - SUB).abs() < 1e-9);
        // Centre→vertex at azimuth 0, centre→edge-midpoint (the apothem) at 60°.
        assert!((centre_to_edge(0.0, g) - g).abs() < 1e-9);
        assert!(centre_to_edge(SUB, g) < g);
    }

    /// A shared edge is placed identically from either direction — the property
    /// the weld depends on. Exercised as the swap the canonicalisation performs.
    #[test]
    fn edge_placement_is_endpoint_order_independent() {
        let (verts, faces) = icosahedron();
        let (a, b) = (verts[faces[0][0]], verts[faces[0][1]]);
        for n in 0..=8u32 {
            let from_a = edge_point(a, b, n, 8);
            let from_b = edge_point(b, a, 8 - n, 8);
            assert_eq!(
                from_a, from_b,
                "site {n}/8 must be bit-identical from both faces"
            );
        }
    }

    /// Edge sites run monotonically from one endpoint to the other and stay on the
    /// great circle between them.
    #[test]
    fn edge_sites_walk_the_arc() {
        let (verts, faces) = icosahedron();
        let (a, b) = (verts[faces[0][0]], verts[faces[0][1]]);
        assert!((edge_point(a, b, 0, 6) - a).length() < 1e-6);
        assert!((edge_point(a, b, 6, 6) - b).length() < 1e-6);
        let mut prev = -1.0f32;
        for n in 0..=6u32 {
            let p = edge_point(a, b, n, 6);
            assert!((p.length() - 1.0).abs() < 1e-5);
            // On the plane of the great circle through a and b.
            assert!(p.dot(a.cross(b)).abs() < 1e-5, "site {n} off the arc");
            let d = a.dot(p).clamp(-1.0, 1.0).acos();
            assert!(d > prev, "site {n} did not advance along the arc");
            prev = d;
        }
    }




    /// Area of the geodesic triangle `(p, q, r)` by Girard's excess.
    fn excess(p: DVec3, q: DVec3, r: DVec3) -> f64 {
        let ang = |u: DVec3, v: DVec3, w: DVec3| {
            let t = |x: DVec3| (x - u * u.dot(x)).normalize();
            t(v).dot(t(w)).clamp(-1.0, 1.0).acos()
        };
        ang(p, q, r) + ang(q, r, p) + ang(r, p, q) - std::f64::consts::PI
    }

    /// The local area scale of the map at barycentric `(fi, fj)`, as a multiple of
    /// the ideal — 1.0 exactly where the map is area-true.
    fn area_scale(frame: &FaceFrame, fi: f64, fj: f64) -> f64 {
        let m = 3000u32;
        let at = |i: u32, j: u32| {
            lattice_point(frame, i, j, m - i - j, m)
                .as_dvec3()
                .normalize()
        };
        let (i, j) = ((fi * m as f64) as u32, (fj * m as f64) as u32);
        let ideal = 4.0 * std::f64::consts::PI / 20.0 / (m * m) as f64;
        excess(at(i, j), at(i + 1, j), at(i, j + 1)) / ideal
    }

    /// **The property the whole projection exists for.** Sampled across a face,
    /// the map carries equal planar area to equal spherical area — so equal
    /// element mass means equal concentration, and one hex step is the same
    /// distance wherever a column takes it.
    #[test]
    fn the_map_is_area_true() {
        let (verts, faces) = icosahedron();
        let f = faces[0];
        let frame = FaceFrame::new(verts[f[0]], verts[f[1]], verts[f[2]]);
        // Interior samples: near each vertex, along each edge, mid-face, and on
        // the edge-midpoint rays where two sub-triangles meet.
        for &(fi, fj) in &[
            (0.90, 0.05),
            (0.05, 0.90),
            (0.10, 0.12),
            (0.02, 0.50),
            (0.45, 0.45),
            (0.30, 0.35),
            (0.60, 0.20),
            (0.10, 0.45),
            (0.45, 0.10),
            (0.20, 0.70),
        ] {
            let s = area_scale(&frame, fi, fj);
            assert!(
                (s - 1.0).abs() < 5e-3,
                "area scale {s} at barycentric ({fi}, {fj}) — the map is not area-true"
            );
        }
    }

    /// Snyder's map is continuous but **not smooth** where two sub-triangles meet
    /// along a vertex ray: the distance to the face boundary turns a corner there,
    /// and the crease reaches inward from the vertex to the face centre. Cells
    /// sitting on a crease therefore come out slightly small.
    ///
    /// This is a property of ISEA, not a defect, and it is pinned here so a future
    /// reader does not mistake the residual for a regression — and so a genuine
    /// regression (a crease that deepens, or one appearing on the smooth
    /// edge-midpoint rays) fails loudly.
    #[test]
    fn the_only_creases_are_the_vertex_rays() {
        let (verts, faces) = icosahedron();
        let f = faces[0];
        let frame = FaceFrame::new(verts[f[0]], verts[f[1]], verts[f[2]]);
        // A sample straddling the vertex-2 ray (barycentric i == j).
        let on_crease = area_scale(&frame, 0.10, 0.10);
        assert!(
            (0.80..0.90).contains(&on_crease),
            "crease depth {on_crease} moved — ISEA's vertex-ray kink changed"
        );
        // The edge-midpoint rays are the other sub-triangle boundary, and there
        // the boundary distance is at a smooth minimum, so no crease forms.
        for &(fi, fj) in &[(0.45, 0.45), (0.02, 0.50), (0.50, 0.02)] {
            let s = area_scale(&frame, fi, fj);
            assert!(
                (s - 1.0).abs() < 5e-3,
                "unexpected crease ({s}) on the edge-midpoint ray at ({fi}, {fj})"
            );
        }
    }
}
