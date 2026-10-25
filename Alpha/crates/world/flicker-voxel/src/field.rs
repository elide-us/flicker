//! **A field of LOD8 vectors** — the coarsest rung of the sparse ladder laid
//! out as geometry. A 2K hex heightmap's pixel IS a cluster's LOD8 vector
//! (Aaron 2026-10-16: *"the heightmap directly relates to 128ft square quads
//! at the highest LOD"*), and the surface is contoured between the vectors
//! exactly as the cluster system contours everything (`mesh.rs`): ONE
//! vertex per cell — here the cell is the cluster — carried at its
//! min-corner voxel plus its corner offset (the cell's centre by default),
//! and ONE quad across every lattice edge, spanning the four cells' vertices
//! around it. Aaron 2026-10-19: *"a single continuous field of quads with
//! each quad intersection defined by the pixel height … a quad at this
//! scale would be formed by the LOD8 vectors of at least four clusters,
//! same pattern as a single voxel is expressed by the vectors of eight
//! lattice vectors."*
//!
//! **This is not a voxel engine.** The first field (2026-10-16) drew one
//! flat quad per cluster at its height with vertical drop walls between
//! neighbours — cubes at staggered heights. Aaron, 2026-10-19: *"What you
//! have built... is a voxel engine."* That path is gone; the failure is
//! banked in MCP memory. Nothing here touches the cluster/CSG LOD0
//! machinery — a vector is what a cluster IS before anyone comes close
//! enough to resolve it.
//!
//! # Units
//!
//! The engine's own: a VOXEL. A cluster spans [`CLUSTER_DIM`] voxels, a
//! height in metres becomes voxels through [`voxel_span_m`] (half a foot).
//! The field lies on the XZ plane, Y up: a column `x` runs east (+X), a row
//! `y` runs north (−Z), the whole field centred on the origin, so a
//! 2048-wide tile reaches ±262 144 voxels — within f32's centimetre at that
//! reach.
//!
//! # The mesh
//!
//! One vertex a cluster, at the cluster's centre in the plane and its
//! vector's height, with the surface's own normal (central differences over
//! its neighbours' vectors) for the renderer's smooth draw; two triangles
//! over every 2×2 block of present clusters, wound to face +Y; nothing
//! where a cluster is missing — the rim is the mask's. Cut into region
//! meshes, a block in the region of its south-west cluster and the other
//! three clusters' vertices borrowed as an apron, so neighbouring regions
//! meet on the same f32s.

use crate::mesh::{ClusterMesh, ClusterVertex};
use clayengine::{cluster_span_m, CLUSTER_DIM};

/// One voxel's span in metres — [`cluster_span_m`] over [`CLUSTER_DIM`]
/// (half a foot), derived so the scale chain is stated once.
pub fn voxel_span_m() -> f64 {
    cluster_span_m() / f64::from(CLUSTER_DIM)
}

/// One cluster of the field: where it is (column, row), its LOD8 vector as a
/// height in VOXELS, and its colour (RGB888, the renderer's direct-colour word
/// made of it at mesh time).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Quad {
    pub x: u32,
    pub y: u32,
    pub height: f32,
    pub rgb: [u8; 3],
}

/// **A field of LOD8 clusters**: `dim × dim` cluster positions of which the
/// clusters present are listed row-major (the mask's order).
#[derive(Clone, Debug, Default)]
pub struct QuadField {
    /// Columns and rows the field spans (2048 for a tile).
    pub dim: u32,
    pub quads: Vec<Quad>,
    /// Row-major lookup `y * dim + x` → index into `quads`, `u32::MAX` where
    /// no cluster stands.
    index: Vec<u32>,
}

impl QuadField {
    /// Build the field over a `dim × dim` grid: `present(x, y)` says whether
    /// a cluster stands there, `height_m(x, y)` its LOD8 vector in metres
    /// (converted to voxels here), `rgb(x, y)` its colour.
    pub fn build(
        dim: u32,
        present: impl Fn(u32, u32) -> bool,
        height_m: impl Fn(u32, u32) -> f32,
        rgb: impl Fn(u32, u32) -> [u8; 3],
    ) -> Self {
        let per_voxel = voxel_span_m() as f32;
        let n = (dim as usize) * (dim as usize);
        let mut index = vec![u32::MAX; n];
        let mut quads = Vec::new();
        for y in 0..dim {
            for x in 0..dim {
                if present(x, y) {
                    index[(y as usize) * dim as usize + x as usize] = quads.len() as u32;
                    quads.push(Quad {
                        x,
                        y,
                        height: height_m(x, y) / per_voxel,
                        rgb: rgb(x, y),
                    });
                }
            }
        }
        Self { dim, quads, index }
    }

    /// The cluster standing at `(x, y)`, if one does.
    pub fn at(&self, x: u32, y: u32) -> Option<&Quad> {
        self.index_at(x, y).map(|i| &self.quads[i as usize])
    }

    /// The cluster at `(x, y)` as an index into `quads`; `None` off the field
    /// or where the mask is empty.
    fn index_at(&self, x: u32, y: u32) -> Option<u32> {
        if x >= self.dim || y >= self.dim {
            return None;
        }
        let i = self.index[(y as usize) * self.dim as usize + x as usize];
        (i != u32::MAX).then_some(i)
    }

    /// The field's world extent: the X (east) and Z (north, negative) of a
    /// column's or row's SOUTH-WEST corner — the grid centred on the origin,
    /// `CLUSTER_DIM` voxels a cluster.
    pub fn west_x(&self, x: u32) -> f32 {
        (x as f32 - self.dim as f32 * 0.5) * CLUSTER_DIM as f32
    }
    pub fn south_z(&self, y: u32) -> f32 {
        -((y as f32 - self.dim as f32 * 0.5) * CLUSTER_DIM as f32)
    }

    /// Where a cluster's vector stands in the world: the cluster's centre in
    /// the plane (the cell's vertex at its min-corner voxel plus the default
    /// corner, as the contour stores it), its height the vector.
    pub fn vertex_position(&self, q: &Quad) -> [f32; 3] {
        let half = CLUSTER_DIM as f32 * 0.5;
        [self.west_x(q.x) + half, q.height, self.south_z(q.y) - half]
    }

    /// The field's centre and half-extent in voxels — what a camera frames.
    /// `(centre_x, centre_y, centre_z, radius)`: the clusters' bounding box
    /// middle at their mean height, and half its diagonal.
    pub fn framing(&self) -> ([f32; 3], f32) {
        if self.quads.is_empty() {
            return ([0.0; 3], CLUSTER_DIM as f32);
        }
        let (mut xlo, mut xhi, mut ylo, mut yhi) = (u32::MAX, 0u32, u32::MAX, 0u32);
        let mut sum = 0.0f64;
        for q in &self.quads {
            xlo = xlo.min(q.x);
            xhi = xhi.max(q.x + 1);
            ylo = ylo.min(q.y);
            yhi = yhi.max(q.y + 1);
            sum += f64::from(q.height);
        }
        let cx = (self.west_x(xlo) + self.west_x(xhi)) * 0.5;
        let cz = (self.south_z(ylo) + self.south_z(yhi)) * 0.5;
        let cy = (sum / self.quads.len() as f64) as f32;
        let half_w = (self.west_x(xhi) - self.west_x(xlo)) * 0.5;
        let half_d = (self.south_z(ylo) - self.south_z(yhi)) * 0.5;
        ([cx, cy, cz], (half_w * half_w + half_d * half_d).sqrt())
    }

    /// The surface's own normal at a cluster: central differences over its
    /// neighbours' vectors (one-sided at the mask's rim), `(-∂h/∂x, 1,
    /// -∂h/∂z)` with X east and Z north-negative, unit length.
    fn surface_normal(&self, q: &Quad) -> [f32; 3] {
        let d = CLUSTER_DIM as f32;
        let h = |x: u32, y: u32| self.index_at(x, y).map(|i| self.quads[i as usize].height);
        let slope = |lo: Option<f32>, hi: Option<f32>| match (lo, hi) {
            (Some(a), Some(b)) => (b - a) / (2.0 * d),
            (Some(a), None) => (q.height - a) / d,
            (None, Some(b)) => (b - q.height) / d,
            (None, None) => 0.0,
        };
        // East is +X; south (y - 1) is +Z, north (y + 1) is -Z.
        let dhdx = slope(q.x.checked_sub(1).and_then(|x| h(x, q.y)), h(q.x + 1, q.y));
        let dhdz = slope(h(q.x, q.y + 1), q.y.checked_sub(1).and_then(|y| h(q.x, y)));
        let n = [-dhdx, 1.0, -dhdz];
        let len = (n[0] * n[0] + 1.0 + n[2] * n[2]).sqrt();
        [n[0] / len, 1.0 / len, n[2] / len]
    }

    /// A cluster as ONE vertex of the surface: its vector, the surface's
    /// normal there, its colour.
    fn surface_vertex(&self, q: &Quad) -> ClusterVertex {
        ClusterVertex {
            position: self.vertex_position(q),
            normal: self.surface_normal(q),
            material: direct_rgb(q.rgb),
        }
    }

    /// **The field as the contoured surface** — one vertex a cluster at its
    /// LOD8 vector, a quad across every lattice edge spanning four clusters'
    /// vectors: two triangles over each 2×2 block of present clusters,
    /// nothing where one is missing (the rim is the mask's). Cut into
    /// `region × region` cluster meshes, a block in the region of its
    /// south-west cluster, the other three clusters' vertices borrowed as an
    /// apron when they lie beyond it, so neighbouring regions meet on the
    /// same f32s. A region's `quads` counts its blocks.
    pub fn surface_meshes(&self, region: u32) -> Vec<FieldRegion> {
        let region = region.max(1);
        let across = self.dim.div_ceil(region);
        let region_of = |q: &Quad| (q.y / region) * across + q.x / region;
        let mut regions: Vec<FieldRegion> = (0..(across * across))
            .map(|r| FieldRegion {
                rx: r % across,
                ry: r / across,
                mesh: ClusterMesh::default(),
                quads: 0,
            })
            .collect();
        // A region's vertex for a cluster, stamped by region so one map
        // serves every region without a clear between them.
        let mut local = vec![u32::MAX; self.quads.len()];
        let mut stamp = vec![u32::MAX; self.quads.len()];
        fn claim(
            mesh: &mut ClusterMesh,
            r: u32,
            i: u32,
            v: ClusterVertex,
            local: &mut [u32],
            stamp: &mut [u32],
        ) -> u32 {
            let i = i as usize;
            if stamp[i] != r {
                stamp[i] = r;
                local[i] = mesh.vertices.len() as u32;
                mesh.vertices.push(v);
            }
            local[i]
        }
        for (i, q) in self.quads.iter().enumerate() {
            let (Some(e), Some(n), Some(ne)) = (
                self.index_at(q.x + 1, q.y),
                self.index_at(q.x, q.y + 1),
                self.index_at(q.x + 1, q.y + 1),
            ) else {
                continue;
            };
            let r = region_of(q);
            let mesh = &mut regions[r as usize].mesh;
            let mut corner = |k: u32| {
                claim(
                    mesh,
                    r,
                    k,
                    self.surface_vertex(&self.quads[k as usize]),
                    &mut local,
                    &mut stamp,
                )
            };
            let (sw, se, ne, nw) = (corner(i as u32), corner(e), corner(ne), corner(n));
            // CCW from above (+Y): SW, SE, NE · SW, NE, NW.
            mesh.indices.extend([sw, se, ne, sw, ne, nw]);
            regions[r as usize].quads += 1;
        }
        regions.retain(|r| r.quads > 0);
        regions
    }
}

/// The renderer's DIRECT-colour material word (bit 31, RGB888 in bits 0–23)
/// — the same escape the globe's shells use; stated here so a field vertex
/// needs no palette slot.
pub fn direct_rgb(rgb: [u8; 3]) -> u32 {
    0x8000_0000 | u32::from(rgb[0]) | (u32::from(rgb[1]) << 8) | (u32::from(rgb[2]) << 16)
}

/// One region's mesh of the surface: which `region × region` block of
/// clusters it covers, the mesh (world voxels), and its quad count.
#[derive(Clone, Debug, Default)]
pub struct FieldRegion {
    pub rx: u32,
    pub ry: u32,
    pub mesh: ClusterMesh,
    pub quads: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A small round field over a 12×12 grid with a height that steps and
    /// rolls, and a hole in the middle.
    fn sample() -> QuadField {
        let dim = 12u32;
        let present = |x: u32, y: u32| {
            let (dx, dy) = (x as f32 - 5.5, y as f32 - 5.5);
            let r2 = dx * dx + dy * dy;
            r2 <= 30.0 && r2 > 2.0
        };
        let height = |x: u32, y: u32| 100.0 + (x as f32 * 0.7).sin() * 3.0 + (y / 3) as f32 * 2.0;
        let rgb = |x: u32, y: u32| [x as u8 * 20, y as u8 * 20, 7];
        QuadField::build(dim, present, height, rgb)
    }

    /// **The vector count is the mask's count** and every cluster carries
    /// its pixel's height, in voxels, and its colour; `at` finds it and
    /// nothing else.
    #[test]
    fn a_vector_per_present_cluster_at_its_own_height() {
        let f = sample();
        let per_voxel = voxel_span_m() as f32;
        let present = (0..12u32)
            .flat_map(|y| (0..12u32).map(move |x| (x, y)))
            .filter(|(x, y)| {
                let (dx, dy) = (*x as f32 - 5.5, *y as f32 - 5.5);
                let r2 = dx * dx + dy * dy;
                r2 <= 30.0 && r2 > 2.0
            })
            .count();
        assert_eq!(f.quads.len(), present);
        for q in &f.quads {
            let want = 100.0 + (q.x as f32 * 0.7).sin() * 3.0 + (q.y / 3) as f32 * 2.0;
            assert_eq!(q.height, want / per_voxel);
            assert_eq!(q.rgb, [q.x as u8 * 20, q.y as u8 * 20, 7]);
            assert_eq!(f.at(q.x, q.y), Some(q));
        }
        assert!(f.at(5, 5).is_none(), "the hole");
        assert!(f.at(12, 0).is_none(), "off the field");
    }

    /// **The surface reads four vectors a quad**: one vertex a cluster, at
    /// its centre and its LOD8 vector, two triangles over every 2×2 block of
    /// present clusters and none where one is missing, every triangle facing
    /// up, the normal leaning against the slope; regions meet on the same
    /// positions.
    #[test]
    fn the_surface_spans_four_vectors_a_quad() {
        let f = sample();
        let regions = f.surface_meshes(4);
        let blocks = f
            .quads
            .iter()
            .filter(|q| {
                f.at(q.x + 1, q.y).is_some()
                    && f.at(q.x, q.y + 1).is_some()
                    && f.at(q.x + 1, q.y + 1).is_some()
            })
            .count();
        assert!(
            blocks > 0 && blocks < f.quads.len(),
            "the hole and the rim cost blocks"
        );
        assert_eq!(regions.iter().map(|r| r.quads).sum::<usize>(), blocks);
        assert_eq!(
            regions.iter().map(|r| r.mesh.indices.len()).sum::<usize>(),
            blocks * 6
        );
        let d = CLUSTER_DIM as f32;
        let mut apron = 0usize;
        for r in &regions {
            for v in &r.mesh.vertices {
                let x = (v.position[0] / d + f.dim as f32 * 0.5 - 0.5).round() as u32;
                let y = (-v.position[2] / d + f.dim as f32 * 0.5 - 0.5).round() as u32;
                let q = f
                    .at(x, y)
                    .expect("a vertex is a present cluster at its centre");
                assert_eq!(v.position, f.vertex_position(q));
                assert_eq!(v.material, direct_rgb(q.rgb));
                assert!(v.normal[1] > 0.0);
                let len = v.normal.iter().map(|c| c * c).sum::<f32>().sqrt();
                assert!((len - 1.0).abs() < 1e-5);
                if x / 4 != r.rx || y / 4 != r.ry {
                    apron += 1;
                }
            }
            for t in r.mesh.indices.chunks(3) {
                let p = |k: u32| r.mesh.vertices[k as usize].position;
                let (a, b, c) = (p(t[0]), p(t[1]), p(t[2]));
                let u = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
                let w = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
                assert!(u[2] * w[0] - u[0] * w[2] > 0.0, "triangle {t:?} faces up");
            }
        }
        assert!(
            apron > 0,
            "blocks across a region's edge borrow the far vertices"
        );
        // The slope leans the normal: a rise to the east tips it west.
        let q = f
            .quads
            .iter()
            .find(|q| q.x > 0 && f.at(q.x + 1, q.y).is_some() && f.at(q.x - 1, q.y).is_some())
            .unwrap();
        let (w, e) = (
            f.at(q.x - 1, q.y).unwrap().height,
            f.at(q.x + 1, q.y).unwrap().height,
        );
        let n = f.surface_normal(q);
        assert_eq!(n[0] > 0.0, e < w, "the normal leans against the slope");
    }

    /// **The framing is the field's box**: its centre the box's middle at
    /// the mean height, its radius half the diagonal; an empty field frames
    /// one cluster at the origin.
    #[test]
    fn the_framing_is_the_field_box() {
        let f = sample();
        let (c, r) = f.framing();
        let (xlo, xhi) = (
            f.quads.iter().map(|q| q.x).min().unwrap(),
            f.quads.iter().map(|q| q.x).max().unwrap() + 1,
        );
        let (ylo, yhi) = (
            f.quads.iter().map(|q| q.y).min().unwrap(),
            f.quads.iter().map(|q| q.y).max().unwrap() + 1,
        );
        assert_eq!(c[0], (f.west_x(xlo) + f.west_x(xhi)) * 0.5);
        assert_eq!(c[2], (f.south_z(ylo) + f.south_z(yhi)) * 0.5);
        let mean = f.quads.iter().map(|q| f64::from(q.height)).sum::<f64>() / f.quads.len() as f64;
        assert!((c[1] - mean as f32).abs() < 1e-3);
        let (hw, hd) = (
            (f.west_x(xhi) - f.west_x(xlo)) * 0.5,
            (f.south_z(ylo) - f.south_z(yhi)) * 0.5,
        );
        assert!((r - (hw * hw + hd * hd).sqrt()).abs() < 1e-3);
        let empty = QuadField::build(4, |_, _| false, |_, _| 0.0, |_, _| [0; 3]);
        assert_eq!(empty.framing(), ([0.0; 3], CLUSTER_DIM as f32));
    }
}
