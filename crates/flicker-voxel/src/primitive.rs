//! Importable primitives and the contour's input contract.
//!
//! A *primitive* is a surface *source* the contour pass queries — distinct
//! from the baked voxel data the contour produces. The pipeline is:
//! **primitive → contour to voxel (QEF) → cluster from voxel data.**
//!
//! The contour consumes a primitive through the [`Primitive`] trait — a
//! Hermite-data contract: classify a grid sample as inside/outside, and
//! for a surface-crossing edge return the crossing position and surface
//! normal. Anything that can answer those two questions plugs into the
//! contour without the contour knowing or caring whether the source is a
//! flat field, an analytic SDF (sphere/box/pill), or an imported mesh.
//!
//! [`FlatField`] is the simplest implementor — a horizontal plane — used
//! as the first known-correct contour input: it should contour to a flat
//! surface at its height with every vertex normal pointing straight up.

use crate::cluster::CLUSTER_DIM;
use crate::heightmap::{world_height_seeded, DEFAULT_SEED};

/// Surface height of the flat stub field, in voxel units: the cluster's
/// vertical midpoint (`CLUSTER_DIM / 2` = 128 for a 256³ cluster), i.e.
/// normalized height 0.5.
pub const FLAT_HEIGHT: f32 = CLUSTER_DIM as f32 / 2.0;

/// Hermite data for one surface crossing: where the surface intersects a
/// grid edge, and the surface normal there. Positions are in cluster-local
/// voxel units, in the same **origin-based** sample frame as
/// [`Primitive::is_solid`] — integer coordinates address the voxel's
/// minimum corner, not its center.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Hermite {
    pub position: [f32; 3],
    pub normal: [f32; 3],
}

/// The contour's input contract — a surface the contour pass can query.
///
/// **Sampling convention (locked).** Grid samples sit at integer voxel
/// **origins** `(x, y, z)`, not centers. A cell `(cx, cy, cz)` spans the
/// unit cube `[cx, cx+1]³` and its 8 corners are voxels `(cx + i, cy + j,
/// cz + k)` for `i, j, k ∈ {0, 1}`. The dual vertex `(0.5, 0.5, 0.5)` then
/// sits at the cell's geometric center, which is the right default for
/// inactive cells.
///
/// Implementors answer two questions:
/// - [`is_solid`](Primitive::is_solid): is the voxel grid sample at integer
///   coords inside the solid? (Classified at the voxel's origin.)
/// - [`edge_hermite`](Primitive::edge_hermite): for an edge between two
///   adjacent grid samples of differing solidity, where does the surface
///   cross and what is its normal?
///
/// Coordinates may be queried outside `[0, CLUSTER_DIM)` (the contour asks
/// about a boundary voxel's neighbors); a primitive defined globally should
/// answer truthfully there so cluster-boundary faces are not spuriously
/// exposed.
pub trait Primitive {
    /// Inside-solid test for the voxel at integer grid coords.
    fn is_solid(&self, x: i32, y: i32, z: i32) -> bool;

    /// Hermite data for the crossing on the edge between adjacent grid
    /// samples `a` and `b`, which the caller guarantees differ in solidity.
    fn edge_hermite(&self, a: [i32; 3], b: [i32; 3]) -> Hermite;
}

/// A flat horizontal field: solid below `height`, empty at or above it.
/// The faked "solid grey field" primitive — contours to a flat plane.
#[derive(Copy, Clone, Debug)]
pub struct FlatField {
    /// Surface height in voxel units.
    pub height: f32,
}

impl FlatField {
    /// A flat field at the cluster's half-height ([`FLAT_HEIGHT`]).
    #[must_use]
    pub fn at_half() -> Self {
        Self {
            height: FLAT_HEIGHT,
        }
    }
}

impl Primitive for FlatField {
    fn is_solid(&self, _x: i32, y: i32, _z: i32) -> bool {
        // Classify at the voxel origin (locked sampling convention).
        (y as f32) < self.height
    }

    fn edge_hermite(&self, a: [i32; 3], b: [i32; 3]) -> Hermite {
        let ca = [a[0] as f32, a[1] as f32, a[2] as f32];
        let cb = [b[0] as f32, b[1] as f32, b[2] as f32];
        // Implicit field f(p) = p.y - height (negative below the surface).
        let fa = ca[1] - self.height;
        let fb = cb[1] - self.height;
        let denom = fa - fb;
        let t = if denom.abs() > 1e-6 { fa / denom } else { 0.5 };
        let position = [
            ca[0] + t * (cb[0] - ca[0]),
            ca[1] + t * (cb[1] - ca[1]),
            ca[2] + t * (cb[2] - ca[2]),
        ];
        // Gradient of f is (0, 1, 0): the surface faces straight up.
        Hermite {
            position,
            normal: [0.0, 1.0, 0.0],
        }
    }
}

/// The foundation-layer terrain primitive: a 2D heightmap sampled from
/// [`crate::heightmap`]. Solid below the surface, empty above. This is
/// the first sloped (non-flat) primitive — the test input that flushes
/// out per-cell-vs-per-solid-voxel storage bugs (heights vary smoothly,
/// so most cells have empty min-corner voxels carrying meaningful
/// vertices).
///
/// The cluster's `CLUSTER_DIM × CLUSTER_DIM` column of heights is cached
/// at construction (~65K samples). Coordinates outside that footprint are
/// resolved on the fly through [`world_height_seeded`] — the heightmap
/// is globally continuous, so neighbor-cluster border faces aren't
/// spuriously exposed.
#[derive(Clone, Debug)]
pub struct HeightField {
    seed: u64,
    offset: [f32; 3],
    /// Row-major `[CLUSTER_DIM × CLUSTER_DIM]` column heights at the
    /// cluster's local `(x, z)` origins (world `(offset.x + x, offset.z + z)`).
    heights: Vec<f32>,
}

impl HeightField {
    /// Sample the cluster's surface column at every cluster-local `(x, z)`
    /// origin with the given `seed`, storing world coordinates relative to
    /// `offset` (only the `x` and `z` components are used).
    #[must_use]
    pub fn new(seed: u64, offset: [f32; 3]) -> Self {
        let dim = CLUSTER_DIM as usize;
        let mut heights = vec![0.0_f32; dim * dim];
        for z in 0..dim {
            for x in 0..dim {
                heights[z * dim + x] = world_height_seeded(
                    offset[0] + x as f32,
                    offset[2] + z as f32,
                    seed,
                );
            }
        }
        Self {
            seed,
            offset,
            heights,
        }
    }

    /// Construct with the heightmap's [`DEFAULT_SEED`].
    #[must_use]
    pub fn from_default_seed(offset: [f32; 3]) -> Self {
        Self::new(DEFAULT_SEED, offset)
    }

    /// Surface height at column `(x, z)`. Cached for `(x, z)` in
    /// `[0, CLUSTER_DIM)`; outside the cluster footprint, falls back to
    /// [`world_height_seeded`] on the fly.
    #[must_use]
    pub fn height_at(&self, x: i32, z: i32) -> f32 {
        let dim = CLUSTER_DIM as i32;
        if x >= 0 && x < dim && z >= 0 && z < dim {
            let stride = CLUSTER_DIM as usize;
            self.heights[z as usize * stride + x as usize]
        } else {
            world_height_seeded(
                self.offset[0] + x as f32,
                self.offset[2] + z as f32,
                self.seed,
            )
        }
    }
}

impl Primitive for HeightField {
    fn is_solid(&self, x: i32, y: i32, z: i32) -> bool {
        (y as f32) < self.height_at(x, z)
    }

    fn edge_hermite(&self, a: [i32; 3], b: [i32; 3]) -> Hermite {
        // Implicit field f(p) = p.y - h(p.x, p.z); the sampled f-value at
        // each endpoint uses *that* endpoint's column height. For an X-
        // or Z-edge, ha and hb generally differ, which is exactly how the
        // slope encodes itself into the crossing position.
        let ha = self.height_at(a[0], a[2]);
        let hb = self.height_at(b[0], b[2]);
        let fa = a[1] as f32 - ha;
        let fb = b[1] as f32 - hb;
        let denom = fa - fb;
        let t = if denom.abs() > 1e-6 { fa / denom } else { 0.5 };
        let position = [
            a[0] as f32 + t * (b[0] - a[0]) as f32,
            a[1] as f32 + t * (b[1] - a[1]) as f32,
            a[2] as f32 + t * (b[2] - a[2]) as f32,
        ];
        // Normal from local height gradient at the crossing column.
        // Central differences with δ=1 over the cached heights — out-of-
        // range lookups fall through to the procedural sampler so the
        // gradient is well-defined at the cluster border too.
        let xc = position[0].round() as i32;
        let zc = position[2].round() as i32;
        let dhdx = (self.height_at(xc + 1, zc) - self.height_at(xc - 1, zc)) * 0.5;
        let dhdz = (self.height_at(xc, zc + 1) - self.height_at(xc, zc - 1)) * 0.5;
        let nx = -dhdx;
        let ny = 1.0_f32;
        let nz = -dhdz;
        let len = (nx * nx + ny * ny + nz * nz).sqrt();
        Hermite {
            position,
            normal: [nx / len, ny / len, nz / len],
        }
    }
}

/// Signed-distance field: negative inside, positive outside, 0 on surface.
///
/// Plugged into the contour via the [`impl_sdf_primitive!`] macro, which
/// derives a [`Primitive`] impl from a distance function alone:
/// inside/outside is `distance < 0`, and edge Hermite data is the linear
/// crossing of the field plus a central-difference gradient normal.
///
/// Object-safe so [`Scene`] can hold a `Vec<Box<dyn Sdf>>` union.
pub trait Sdf {
    /// Signed distance from `p` (in cluster-local voxel units) to the
    /// surface — negative inside the solid.
    fn distance(&self, p: [f32; 3]) -> f32;
}

/// Central-difference step (in voxels) for the gradient normal computed
/// by [`sdf_hermite`]. Half a voxel: small enough to stay local, large
/// enough that the SDF varies meaningfully across the stencil.
const SDF_EPS: f32 = 0.5;

#[inline]
fn len3(v: [f32; 3]) -> f32 {
    (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt()
}

#[inline]
fn len2(x: f32, y: f32) -> f32 {
    (x * x + y * y).sqrt()
}

/// Hermite data for an edge between two grid origins, derived from any
/// signed-distance function. Crossing position is the linear interpolant
/// where the field hits zero; normal is the normalized outward gradient
/// (central differences) at that point.
fn sdf_hermite(distance: impl Fn([f32; 3]) -> f32, a: [i32; 3], b: [i32; 3]) -> Hermite {
    let pa = [a[0] as f32, a[1] as f32, a[2] as f32];
    let pb = [b[0] as f32, b[1] as f32, b[2] as f32];
    let da = distance(pa);
    let db = distance(pb);
    let denom = da - db;
    let t = if denom.abs() > 1e-6 { da / denom } else { 0.5 };
    let pos = [
        pa[0] + t * (pb[0] - pa[0]),
        pa[1] + t * (pb[1] - pa[1]),
        pa[2] + t * (pb[2] - pa[2]),
    ];
    let e = SDF_EPS;
    let gx = distance([pos[0] + e, pos[1], pos[2]]) - distance([pos[0] - e, pos[1], pos[2]]);
    let gy = distance([pos[0], pos[1] + e, pos[2]]) - distance([pos[0], pos[1] - e, pos[2]]);
    let gz = distance([pos[0], pos[1], pos[2] + e]) - distance([pos[0], pos[1], pos[2] - e]);
    let len = (gx * gx + gy * gy + gz * gz).sqrt();
    let normal = if len > 1e-12 {
        [gx / len, gy / len, gz / len]
    } else {
        [0.0, 1.0, 0.0]
    };
    Hermite {
        position: pos,
        normal,
    }
}

/// Derive [`Primitive`] for a type that already impls [`Sdf`]. Inside-test
/// is `distance < 0`; edge Hermite is the linear crossing + central-
/// difference normal via [`sdf_hermite`].
///
/// Done as a macro rather than a blanket `impl<T: Sdf> Primitive for T` to
/// avoid a coherence conflict with the direct `Primitive` impls on
/// [`FlatField`] / [`HeightField`], neither of which implements `Sdf`.
macro_rules! impl_sdf_primitive {
    ($t:ty) => {
        impl Primitive for $t {
            fn is_solid(&self, x: i32, y: i32, z: i32) -> bool {
                <Self as Sdf>::distance(self, [x as f32, y as f32, z as f32]) < 0.0
            }
            fn edge_hermite(&self, a: [i32; 3], b: [i32; 3]) -> Hermite {
                sdf_hermite(|p| <Self as Sdf>::distance(self, p), a, b)
            }
        }
    };
}

/// Sphere centered at `center` with radius `radius`. The smoothest shape
/// in the gallery — the cleanest visual check that DC normals come out
/// right on a curved surface.
#[derive(Copy, Clone, Debug)]
pub struct Sphere {
    pub center: [f32; 3],
    pub radius: f32,
}

impl Sdf for Sphere {
    fn distance(&self, p: [f32; 3]) -> f32 {
        len3([
            p[0] - self.center[0],
            p[1] - self.center[1],
            p[2] - self.center[2],
        ]) - self.radius
    }
}
impl_sdf_primitive!(Sphere);

/// Axis-aligned box centered at `center` with axis half-extents `half`.
/// Edges and corners facet under single-vertex-per-cell DC — that's the
/// expected sharp-feature artifact, not a bug.
#[derive(Copy, Clone, Debug)]
pub struct Cube {
    pub center: [f32; 3],
    pub half: [f32; 3],
}

impl Sdf for Cube {
    fn distance(&self, p: [f32; 3]) -> f32 {
        let qx = (p[0] - self.center[0]).abs() - self.half[0];
        let qy = (p[1] - self.center[1]).abs() - self.half[1];
        let qz = (p[2] - self.center[2]).abs() - self.half[2];
        let outside = len3([qx.max(0.0), qy.max(0.0), qz.max(0.0)]);
        let inside = qx.max(qy).max(qz).min(0.0);
        outside + inside
    }
}
impl_sdf_primitive!(Cube);

#[inline]
fn cylinder_distance(p: [f32; 3], center: [f32; 3], radius: f32, half_height: f32) -> f32 {
    let radial = len2(p[0] - center[0], p[2] - center[2]) - radius;
    let dy = (p[1] - center[1]).abs() - half_height;
    let outside = len2(radial.max(0.0), dy.max(0.0));
    let inside = radial.max(dy).min(0.0);
    outside + inside
}

/// Y-axis capped cylinder centered at `center`. The cap rims facet under
/// DC — same single-vertex-per-cell sharp-feature artifact.
#[derive(Copy, Clone, Debug)]
pub struct Cylinder {
    pub center: [f32; 3],
    pub radius: f32,
    pub half_height: f32,
}

impl Sdf for Cylinder {
    fn distance(&self, p: [f32; 3]) -> f32 {
        cylinder_distance(p, self.center, self.radius, self.half_height)
    }
}
impl_sdf_primitive!(Cylinder);

/// Y-axis capped cone with the apex pointing +Y. The base sits at y =
/// `center.y - half_height` with radius `base_radius`; the apex sits at
/// y = `center.y + half_height` (radius 0). Inigo Quilez's capped-cone
/// SDF specialized to `r2 = 0`. Apex point and base rim facet.
#[derive(Copy, Clone, Debug)]
pub struct Cone {
    pub center: [f32; 3],
    pub base_radius: f32,
    pub half_height: f32,
}

impl Sdf for Cone {
    fn distance(&self, p: [f32; 3]) -> f32 {
        let r1 = self.base_radius;
        let r2 = 0.0_f32;
        let h = self.half_height;
        let qx = len2(p[0] - self.center[0], p[2] - self.center[2]);
        let qy = p[1] - self.center[1];

        let k1x = r2;
        let k1y = h;
        let k2x = r2 - r1;
        let k2y = 2.0 * h;

        let r_at_y = if qy < 0.0 { r1 } else { r2 };
        let ca_x = qx - qx.min(r_at_y);
        let ca_y = qy.abs() - h;

        let cl_num = (k1x - qx) * k2x + (k1y - qy) * k2y;
        let cl_den = k2x * k2x + k2y * k2y;
        let cl = (cl_num / cl_den).clamp(0.0, 1.0);
        let cb_x = qx - k1x + k2x * cl;
        let cb_y = qy - k1y + k2y * cl;

        let s = if cb_x < 0.0 && ca_y < 0.0 { -1.0 } else { 1.0 };
        let m = (ca_x * ca_x + ca_y * ca_y).min(cb_x * cb_x + cb_y * cb_y);
        s * m.sqrt()
    }
}
impl_sdf_primitive!(Cone);

/// Upper hemisphere (dome) of a sphere, with the flat base sitting on
/// the plane y = `center.y`. Composed as `max(sphere, base_plane)`; the
/// dome/base seam facets — finite-difference normals don't preserve the
/// sharp rim.
#[derive(Copy, Clone, Debug)]
pub struct HalfSphere {
    pub center: [f32; 3],
    pub radius: f32,
}

impl Sdf for HalfSphere {
    fn distance(&self, p: [f32; 3]) -> f32 {
        let s = len3([
            p[0] - self.center[0],
            p[1] - self.center[1],
            p[2] - self.center[2],
        ]) - self.radius;
        let h = self.center[1] - p[1];
        s.max(h)
    }
}
impl_sdf_primitive!(HalfSphere);

/// Y-axis cylinder sliced by the plane x = `center.x`, keeping the half-
/// volume with x ≤ `center.x`. The cut rim facets — same single-vertex-
/// per-cell DC limitation as the other sharp-featured primitives.
#[derive(Copy, Clone, Debug)]
pub struct HalfCylinder {
    pub center: [f32; 3],
    pub radius: f32,
    pub half_height: f32,
}

impl Sdf for HalfCylinder {
    fn distance(&self, p: [f32; 3]) -> f32 {
        let c = cylinder_distance(p, self.center, self.radius, self.half_height);
        let h = p[0] - self.center[0];
        c.max(h)
    }
}
impl_sdf_primitive!(HalfCylinder);

/// Union of arbitrary [`Sdf`] parts, contoured in one pass. The composite
/// distance is the min over the parts — every part is implicitly merged
/// without overlap-resolution logic. Built so the example can show all
/// six analytic primitives in a single contour.
pub struct Scene {
    parts: Vec<Box<dyn Sdf>>,
}

impl Scene {
    /// A 3×2 XZ grid of the six analytic primitives at y=128 inside the
    /// 256³ cluster, sized with margins so no two shapes overlap.
    /// Row z=96: sphere, cube, cylinder. Row z=160: half-sphere (dome),
    /// half-cylinder, cone.
    #[must_use]
    pub fn gallery() -> Self {
        let parts: Vec<Box<dyn Sdf>> = vec![
            Box::new(Sphere {
                center: [64.0, 128.0, 96.0],
                radius: 30.0,
            }),
            Box::new(Cube {
                center: [128.0, 128.0, 96.0],
                half: [28.0, 28.0, 28.0],
            }),
            Box::new(Cylinder {
                center: [192.0, 128.0, 96.0],
                radius: 26.0,
                half_height: 34.0,
            }),
            Box::new(HalfSphere {
                center: [64.0, 128.0, 160.0],
                radius: 32.0,
            }),
            Box::new(HalfCylinder {
                center: [128.0, 128.0, 160.0],
                radius: 28.0,
                half_height: 34.0,
            }),
            Box::new(Cone {
                center: [192.0, 128.0, 160.0],
                base_radius: 30.0,
                half_height: 34.0,
            }),
        ];
        Self { parts }
    }
}

impl Sdf for Scene {
    fn distance(&self, p: [f32; 3]) -> f32 {
        self.parts
            .iter()
            .map(|s| s.distance(p))
            .fold(f32::INFINITY, f32::min)
    }
}
impl_sdf_primitive!(Scene);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flat_field_solidity_boundary() {
        let f = FlatField::at_half(); // height 128
        // Origin of voxel 127 is 127 < 128 → solid.
        assert!(f.is_solid(10, 127, 10));
        // Origin of voxel 128 is 128 ≥ 128 → empty.
        assert!(!f.is_solid(10, 128, 10));
        // Below stays solid; well above stays empty.
        assert!(f.is_solid(10, 0, 10));
        assert!(!f.is_solid(10, 255, 10));
    }

    #[test]
    fn flat_field_edge_crossing_is_on_plane_with_up_normal() {
        let f = FlatField::at_half();
        // The +Y edge of the surface voxel (10, 127, 10) crosses at the
        // plane y=128 in origin coords, which coincides with the upper
        // endpoint.
        let h = f.edge_hermite([10, 127, 10], [10, 128, 10]);
        assert!((h.position[0] - 10.0).abs() < 1e-4);
        assert!((h.position[1] - 128.0).abs() < 1e-4);
        assert!((h.position[2] - 10.0).abs() < 1e-4);
        assert_eq!(h.normal, [0.0, 1.0, 0.0]);
    }

    #[test]
    fn heightfield_is_solid_matches_height() {
        let f = HeightField::from_default_seed([0.0, 0.0, 0.0]);
        let x = 42;
        let z = 137;
        let h = f.height_at(x, z);
        // Heights live in [64, 192] (BASE ± AMPLITUDE), well clear of the
        // integer grid extremes — clearly-below and clearly-above probes
        // never collide with the column.
        assert!(f.is_solid(x, (h - 5.0) as i32, z));
        assert!(!f.is_solid(x, (h + 5.0) as i32, z));
        // Boundary: the voxel just below h is solid; the one just above
        // is empty (origin-based: is_solid(y) iff y < h).
        let below = h.floor() as i32 - 1;
        let above = h.ceil() as i32;
        assert!(f.is_solid(x, below, z), "y={below} should be solid for h={h}");
        assert!(
            !f.is_solid(x, above, z),
            "y={above} should be empty for h={h}"
        );
    }

    #[test]
    fn heightfield_vertical_edge_crosses_at_surface() {
        let f = HeightField::from_default_seed([0.0, 0.0, 0.0]);
        let x = 42_i32;
        let z = 137_i32;
        let h = f.height_at(x, z);
        // A +Y edge that straddles the surface for this column.
        let y0 = h.floor() as i32;
        let y1 = y0 + 1;
        assert!(
            f.is_solid(x, y0, z) && !f.is_solid(x, y1, z),
            "edge ({y0}..{y1}) must straddle h={h} for this test to be meaningful"
        );

        let hh = f.edge_hermite([x, y0, z], [x, y1, z]);
        assert!(
            (hh.position[1] - h).abs() < 0.01,
            "crossing y={} should equal column height h={h}",
            hh.position[1]
        );
        assert!((hh.position[0] - x as f32).abs() < 1e-4);
        assert!((hh.position[2] - z as f32).abs() < 1e-4);

        // Normal is unit-length and dominated by +Y for any reasonable
        // local slope; gradient sign should match the actual height
        // difference along each axis (small but non-zero in general).
        let n = hh.normal;
        let mag = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
        assert!((mag - 1.0).abs() < 1e-3, "normal not unit: {n:?}");
        assert!(n[1] > 0.5, "ny={} should be +Y dominant", n[1]);

        let dh_x = f.height_at(x + 1, z) - f.height_at(x - 1, z);
        if dh_x.abs() > 0.05 {
            assert!(
                n[0].signum() == (-dh_x).signum(),
                "nx should oppose +X gradient (dh_x={dh_x}, nx={})",
                n[0]
            );
        }
        let dh_z = f.height_at(x, z + 1) - f.height_at(x, z - 1);
        if dh_z.abs() > 0.05 {
            assert!(
                n[2].signum() == (-dh_z).signum(),
                "nz should oppose +Z gradient (dh_z={dh_z}, nz={})",
                n[2]
            );
        }
    }

    #[test]
    fn sphere_is_solid_sign() {
        let s = Sphere {
            center: [10.0, 10.0, 10.0],
            radius: 5.0,
        };
        assert!(s.is_solid(10, 10, 10));
        assert!(!s.is_solid(100, 100, 100));
    }

    #[test]
    fn sphere_edge_hermite_normal_outward() {
        let s = Sphere {
            center: [10.0, 10.0, 10.0],
            radius: 5.0,
        };
        // (14,10,10) is inside the +X face (dist=-1); (15,10,10) is on it (dist=0).
        let h = s.edge_hermite([14, 10, 10], [15, 10, 10]);
        assert!((h.position[0] - 15.0).abs() < 0.05, "x={}", h.position[0]);
        assert!((h.position[1] - 10.0).abs() < 0.05);
        assert!((h.position[2] - 10.0).abs() < 0.05);
        assert!(h.normal[0] > 0.95, "n={:?}", h.normal);
        assert!(h.normal[1].abs() < 0.1);
        assert!(h.normal[2].abs() < 0.1);
    }

    #[test]
    fn cube_is_solid_sign() {
        let c = Cube {
            center: [10.0, 10.0, 10.0],
            half: [5.0, 5.0, 5.0],
        };
        assert!(c.is_solid(10, 10, 10));
        assert!(!c.is_solid(100, 100, 100));
    }

    #[test]
    fn cube_edge_hermite_normal_outward() {
        let c = Cube {
            center: [10.0, 10.0, 10.0],
            half: [5.0, 5.0, 5.0],
        };
        // +Y face at y=15.
        let h = c.edge_hermite([10, 14, 10], [10, 15, 10]);
        assert!((h.position[1] - 15.0).abs() < 0.05, "y={}", h.position[1]);
        assert!(h.normal[1] > 0.95, "n={:?}", h.normal);
        assert!(h.normal[0].abs() < 0.1);
        assert!(h.normal[2].abs() < 0.1);
    }

    #[test]
    fn cylinder_is_solid_sign() {
        let c = Cylinder {
            center: [10.0, 10.0, 10.0],
            radius: 5.0,
            half_height: 5.0,
        };
        assert!(c.is_solid(10, 10, 10));
        assert!(!c.is_solid(100, 100, 100));
    }

    #[test]
    fn cylinder_edge_hermite_normal_outward() {
        let c = Cylinder {
            center: [10.0, 10.0, 10.0],
            radius: 5.0,
            half_height: 5.0,
        };
        // +X side at the equator: x=15.
        let h = c.edge_hermite([14, 10, 10], [15, 10, 10]);
        assert!((h.position[0] - 15.0).abs() < 0.05, "x={}", h.position[0]);
        assert!(h.normal[0] > 0.95, "n={:?}", h.normal);
        assert!(h.normal[1].abs() < 0.1);
        assert!(h.normal[2].abs() < 0.1);
    }

    #[test]
    fn cone_is_solid_sign() {
        let c = Cone {
            center: [10.0, 10.0, 10.0],
            base_radius: 5.0,
            half_height: 5.0,
        };
        // Interior on the axis between base (y=5) and apex (y=15).
        assert!(c.is_solid(10, 8, 10));
        assert!(!c.is_solid(100, 100, 100));
    }

    #[test]
    fn cone_edge_hermite_normal_outward() {
        let c = Cone {
            center: [10.0, 10.0, 10.0],
            base_radius: 5.0,
            half_height: 5.0,
        };
        // Bottom-cap edge: (10,6,10) inside, (10,5,10) on the cap plane.
        let h = c.edge_hermite([10, 6, 10], [10, 5, 10]);
        assert!((h.position[1] - 5.0).abs() < 0.05, "y={}", h.position[1]);
        // Bottom cap faces -Y.
        assert!(h.normal[1] < -0.95, "n={:?}", h.normal);
    }

    #[test]
    fn half_sphere_is_solid_sign() {
        let h = HalfSphere {
            center: [10.0, 10.0, 10.0],
            radius: 5.0,
        };
        assert!(h.is_solid(10, 12, 10), "dome interior should be solid");
        assert!(!h.is_solid(100, 100, 100), "far exterior should be empty");
        assert!(
            !h.is_solid(10, 9, 10),
            "below the cut plane should be empty"
        );
    }

    #[test]
    fn half_sphere_edge_hermite_normal_outward() {
        let hs = HalfSphere {
            center: [10.0, 10.0, 10.0],
            radius: 5.0,
        };
        // Top of the dome — straight up.
        let h = hs.edge_hermite([10, 14, 10], [10, 15, 10]);
        assert!((h.position[1] - 15.0).abs() < 0.05);
        assert!(h.normal[1] > 0.95, "n={:?}", h.normal);
    }

    #[test]
    fn half_cylinder_is_solid_sign() {
        let c = HalfCylinder {
            center: [10.0, 10.0, 10.0],
            radius: 5.0,
            half_height: 5.0,
        };
        assert!(c.is_solid(8, 10, 10), "interior on x≤center side");
        assert!(!c.is_solid(100, 100, 100), "far exterior should be empty");
        assert!(
            !c.is_solid(11, 10, 10),
            "past the cut plane should be empty"
        );
    }

    #[test]
    fn half_cylinder_edge_hermite_normal_outward() {
        let c = HalfCylinder {
            center: [10.0, 10.0, 10.0],
            radius: 5.0,
            half_height: 5.0,
        };
        // -X side of the cylinder, at x=5.
        let h = c.edge_hermite([6, 10, 10], [5, 10, 10]);
        assert!((h.position[0] - 5.0).abs() < 0.05);
        assert!(h.normal[0] < -0.95, "n={:?}", h.normal);
    }
}
