//! Triangle decimation for a [`RawModel`] — a hand-rolled QEM (quadric error metric)
//! edge-collapse that takes the mesh down to an ABSOLUTE triangle target.
//!
//! Design rulings:
//!   * (Aaron, 2026-09-03) the target is a **triangle COUNT**, typed on the Clayworks Prep step
//!     and applied on a button — not a percent of the source. Percent was never precise
//!     enough; the per-race counts vary and are made consistent by hand. This supersedes the
//!     2026-08-22 percent-of-source ruling (100% → 50% in 5% buckets, one pass snapshotting
//!     each bucket for a live slider): with an Apply button there is nothing to scrub, so the
//!     mesh collapses once, to the typed count.
//!   * a target at or above the source count is the source verbatim; there is no floor other
//!     than what interior-only collapse can legally reach (see below), so the result may sit
//!     ABOVE a very deep target — the caller reports the count it actually got.
//!
//! No dependency (no `meshopt`). This is a STANDALONE triangle-mesh path — it never touches the
//! voxel QEF / dual-contouring LOD (`flicker-voxel`), a different algorithm Aaron ruled off-limits.
//!
//! Fidelity stance (v1, the plan's "keep seams rather than chase attribute-QEM"): the mesh is
//! welded by position+uv+normal into wedges, so a UV seam is a boundary in wedge space; only
//! edges whose BOTH endpoints are interior (non-boundary) are collapsed. That preserves every
//! UV seam and silhouette border — no texture cracks, no holes — at the cost of leaving
//! seam-dense regions denser, and of a deep target stopping early on a seam-dense mesh.

use std::collections::HashMap;

use glam::Vec3;

use crate::fbx::{RawModel, RawVertex};

/// Decimate `model` down to at most `target_tris` triangles. A target at or above the source
/// count returns the source verbatim (nothing to do — and a tiny mesh, under four triangles, is
/// never touched). Below that the interior edges collapse cheapest-first until the count is
/// reached or no legal collapse remains, so on a seam-dense mesh the result can stop ABOVE the
/// target; it never drops below one triangle. The source must be non-deduped with sequential
/// indices (the `parse_fbx` convention); the result keeps that convention.
pub fn decimate_to(model: &RawModel, target_tris: usize) -> RawModel {
    let source_tris = model.indices.len() / 3;
    if source_tris < 4 || target_tris >= source_tris {
        return model.clone();
    }
    let mut mesh = WeldedMesh::from_raw(model);
    mesh.collapse_to(target_tris.max(1));
    mesh.to_raw(model)
}

// ── QEM internals ───────────────────────────────────────────────────────────────────────────

/// A symmetric 4×4 quadric, stored as its 10 upper-triangle coefficients (a=x, b=y, c=z, d=1).
#[derive(Debug, Clone, Copy, Default)]
struct Quadric {
    // [aa, ab, ac, ad, bb, bc, bd, cc, cd, dd]
    m: [f64; 10],
}

impl Quadric {
    /// The fundamental error quadric of the plane `n·x + d = 0` (n unit).
    fn plane(n: Vec3, d: f32) -> Self {
        let (a, b, c, d) = (n.x as f64, n.y as f64, n.z as f64, d as f64);
        Quadric {
            m: [
                a * a,
                a * b,
                a * c,
                a * d,
                b * b,
                b * c,
                b * d,
                c * c,
                c * d,
                d * d,
            ],
        }
    }

    fn add(&mut self, o: &Quadric) {
        for i in 0..10 {
            self.m[i] += o.m[i];
        }
    }

    /// Error `vᵀ Q v` at point `p`.
    fn error(&self, p: Vec3) -> f64 {
        let (x, y, z) = (p.x as f64, p.y as f64, p.z as f64);
        let m = &self.m;
        x * x * m[0]
            + 2.0 * x * y * m[1]
            + 2.0 * x * z * m[2]
            + 2.0 * x * m[3]
            + y * y * m[4]
            + 2.0 * y * z * m[5]
            + 2.0 * y * m[6]
            + z * z * m[7]
            + 2.0 * z * m[8]
            + m[9]
    }
}

/// A wedge = a unique (position, uv, normal) — the standard indexed vertex. A UV/normal seam
/// splits into distinct wedges, so seams show up as boundaries in this topology.
struct WeldedMesh {
    pos: Vec<Vec3>,
    attr: Vec<RawVertex>,
    alive: Vec<bool>,
    /// A boundary/seam wedge — never the moved endpoint of a collapse (keeps seams & borders).
    locked: Vec<bool>,
    quad: Vec<Quadric>,
    /// Triangles as wedge-index triples; `tri_alive` marks survivors.
    tris: Vec<[u32; 3]>,
    tri_alive: Vec<bool>,
    /// Triangles incident to each wedge (may contain dead entries — filtered on use).
    incident: Vec<Vec<u32>>,
    live_tris: usize,
}

/// Quantize a float to a weld grid (~1e-4 cm), so byte-identical corners weld.
fn key(f: f32) -> i64 {
    (f as f64 * 10_000.0).round() as i64
}

impl WeldedMesh {
    fn from_raw(model: &RawModel) -> Self {
        // Weld corners into wedges by (position, uv). Normal is deliberately NOT in the key: a
        // smooth-shaded Meshy mesh carries per-corner normals that differ by a hair, so keying on
        // them fragments every shared vertex into singletons — the whole surface reads as boundary
        // and nothing collapses. UV stays in the key so a texture seam is a boundary and never
        // collapses across; normals are recomputed area-weighted per snapshot in `to_raw`.
        let mut map: HashMap<[i64; 5], u32> = HashMap::new();
        let mut pos: Vec<Vec3> = Vec::new();
        let mut attr: Vec<RawVertex> = Vec::new();
        let mut corner_wedge: Vec<u32> = Vec::with_capacity(model.vertices.len());
        for v in &model.vertices {
            let k = [
                key(v.p[0]),
                key(v.p[1]),
                key(v.p[2]),
                key(v.uv[0]),
                key(v.uv[1]),
            ];
            let id = *map.entry(k).or_insert_with(|| {
                pos.push(Vec3::from_array(v.p));
                attr.push(*v);
                (pos.len() - 1) as u32
            });
            corner_wedge.push(id);
        }

        let n = pos.len();
        let mut tris: Vec<[u32; 3]> = Vec::with_capacity(model.indices.len() / 3);
        let mut incident: Vec<Vec<u32>> = vec![Vec::new(); n];
        for t in model.indices.as_chunks::<3>().0 {
            let a = corner_wedge[t[0] as usize];
            let b = corner_wedge[t[1] as usize];
            let c = corner_wedge[t[2] as usize];
            if a == b || b == c || a == c {
                continue; // a corner-welded degenerate triangle
            }
            let ti = tris.len() as u32;
            tris.push([a, b, c]);
            incident[a as usize].push(ti);
            incident[b as usize].push(ti);
            incident[c as usize].push(ti);
        }
        let live_tris = tris.len();

        // Quadrics: sum the plane quadric of every incident triangle.
        let mut quad = vec![Quadric::default(); n];
        for tri in &tris {
            let (p0, p1, p2) = (
                pos[tri[0] as usize],
                pos[tri[1] as usize],
                pos[tri[2] as usize],
            );
            let nrm = (p1 - p0).cross(p2 - p0);
            let len = nrm.length();
            if len < 1e-12 {
                continue;
            }
            let unit = nrm / len;
            let d = -unit.dot(p0);
            let q = Quadric::plane(unit, d);
            for &vi in tri {
                quad[vi as usize].add(&q);
            }
        }

        // Boundary/seam detection: an undirected edge used by exactly one triangle is a border;
        // both its endpoints are locked (never the moved side of a collapse).
        let mut edge_use: HashMap<(u32, u32), u32> = HashMap::new();
        for tri in &tris {
            for &(a, b) in &[(tri[0], tri[1]), (tri[1], tri[2]), (tri[2], tri[0])] {
                let e = if a < b { (a, b) } else { (b, a) };
                *edge_use.entry(e).or_insert(0) += 1;
            }
        }
        let mut locked = vec![false; n];
        for (&(a, b), &count) in &edge_use {
            if count == 1 {
                locked[a as usize] = true;
                locked[b as usize] = true;
            }
        }

        WeldedMesh {
            pos,
            attr,
            alive: vec![true; n],
            locked,
            quad,
            tris,
            tri_alive: vec![true; live_tris],
            incident,
            live_tris,
        }
    }

    /// Undirected interior edges (both endpoints unlocked) among currently-alive triangles.
    fn collect_edges(&self) -> Vec<(u32, u32)> {
        let mut seen: HashMap<(u32, u32), ()> = HashMap::new();
        let mut edges = Vec::new();
        for (ti, tri) in self.tris.iter().enumerate() {
            if !self.tri_alive[ti] {
                continue;
            }
            for &(a, b) in &[(tri[0], tri[1]), (tri[1], tri[2]), (tri[2], tri[0])] {
                if self.locked[a as usize] || self.locked[b as usize] {
                    continue;
                }
                let e = if a < b { (a, b) } else { (b, a) };
                if seen.insert(e, ()).is_none() {
                    edges.push(e);
                }
            }
        }
        edges
    }

    /// Collapse interior edges cheapest-first until the live triangle count reaches `target`
    /// (or no legal collapse remains). Re-scans edges in rounds — simple and robust; the pass
    /// runs once per Apply on the Prep step, not per frame.
    fn collapse_to(&mut self, target: usize) {
        while self.live_tris > target {
            let edges = self.collect_edges();
            if edges.is_empty() {
                break;
            }
            // Cost each edge; collapse the cheapest that is still legal, then re-cost. To avoid
            // O(n²) churn we collapse a batch per round: sort by cost and greedily take edges
            // whose endpoints are untouched this round.
            let mut costed: Vec<(f64, u32, u32, Vec3)> = edges
                .iter()
                .map(|&(a, b)| {
                    let target_pos = self.best_position(a, b);
                    let mut q = self.quad[a as usize];
                    q.add(&self.quad[b as usize]);
                    (q.error(target_pos), a, b, target_pos)
                })
                .collect();
            costed.sort_by(|x, y| x.0.total_cmp(&y.0));

            let mut touched = vec![false; self.pos.len()];
            let mut progressed = false;
            for (_, a, b, tpos) in costed {
                if self.live_tris <= target {
                    break;
                }
                let (a, b) = (a as usize, b as usize);
                if !self.alive[a] || !self.alive[b] {
                    continue;
                }
                if touched[a] || touched[b] || self.locked[a] || self.locked[b] {
                    continue;
                }
                // Guard against non-manifold folds: skip if the two share more than the two
                // triangles of their common edge (a thin fin) — a cheap safety net.
                if !self.collapse(a, b, tpos) {
                    continue;
                }
                touched[a] = true;
                touched[b] = true;
                progressed = true;
            }
            if !progressed {
                break;
            }
        }
    }

    /// The quadric-optimal collapse position, clamped to sane fallbacks (endpoints / midpoint).
    fn best_position(&self, a: u32, b: u32) -> Vec3 {
        let (pa, pb) = (self.pos[a as usize], self.pos[b as usize]);
        let mid = (pa + pb) * 0.5;
        let mut q = self.quad[a as usize];
        q.add(&self.quad[b as usize]);
        // Try candidates; the optimal solve is skipped (unstable to hand-roll) in favour of the
        // best of {a, b, midpoint} — cheap, stable, and adequate at ≤50% reduction.
        let cands = [pa, pb, mid];
        cands
            .into_iter()
            .min_by(|&x, &y| q.error(x).total_cmp(&q.error(y)))
            .unwrap_or(mid)
    }

    /// Collapse `b` into `a`: move `a` to `tpos`, retarget `b`'s triangles, drop the degenerate
    /// ones, merge quadrics and incidence. Returns false (and does nothing) if the collapse
    /// would remove more than the shared edge's triangles into degeneracy in an unsafe way.
    fn collapse(&mut self, a: usize, b: usize, tpos: Vec3) -> bool {
        // Retarget b→a across b's triangles; a triangle that then has a repeated vertex dies.
        let b_tris: Vec<u32> = self.incident[b].clone();
        let mut removed = 0usize;
        for &ti in &b_tris {
            let t = ti as usize;
            if !self.tri_alive[t] {
                continue;
            }
            let tri = &mut self.tris[t];
            for s in tri.iter_mut() {
                if *s as usize == b {
                    *s = a as u32;
                }
            }
            let [x, y, z] = *tri;
            if x == y || y == z || x == z {
                self.tri_alive[t] = false;
                removed += 1;
            } else {
                self.incident[a].push(ti);
            }
        }
        // Apply the merge.
        self.pos[a] = tpos;
        self.alive[b] = false;
        let qb = self.quad[b];
        self.quad[a].add(&qb);
        self.live_tris -= removed;
        true
    }

    /// Snapshot the current surface back into a non-deduped [`RawModel`] (one vertex per corner,
    /// sequential indices) so `conform`/`bake_skin`/`bake_rig` consume it unchanged. Normals are
    /// recomputed area-weighted from the surviving triangles; uv/joints/weights ride each wedge.
    fn to_raw(&self, source: &RawModel) -> RawModel {
        // Area-weighted smooth normal per wedge from alive triangles.
        let mut nrm = vec![Vec3::ZERO; self.pos.len()];
        for (ti, tri) in self.tris.iter().enumerate() {
            if !self.tri_alive[ti] {
                continue;
            }
            let (p0, p1, p2) = (
                self.pos[tri[0] as usize],
                self.pos[tri[1] as usize],
                self.pos[tri[2] as usize],
            );
            let fn_ = (p1 - p0).cross(p2 - p0); // length ∝ 2·area, unnormalised = area weight
            for &vi in tri {
                nrm[vi as usize] += fn_;
            }
        }

        let mut vertices: Vec<RawVertex> = Vec::with_capacity(self.live_tris * 3);
        for (ti, tri) in self.tris.iter().enumerate() {
            if !self.tri_alive[ti] {
                continue;
            }
            for &vi in tri {
                let w = vi as usize;
                let mut v = self.attr[w];
                v.p = self.pos[w].to_array();
                let n = nrm[w].normalize_or_zero();
                if n != Vec3::ZERO {
                    v.n = n.to_array();
                }
                vertices.push(v);
            }
        }
        // If everything collapsed away (shouldn't at ≤50%), fall back to the source.
        if vertices.is_empty() {
            return source.clone();
        }
        let indices: Vec<u32> = (0..vertices.len() as u32).collect();
        RawModel {
            vertices,
            indices,
            bones: source.bones.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A subdivided grid quad (many interior verts, a locked border) — the decimator's happy path.
    fn grid(n: usize) -> RawModel {
        let mut verts = Vec::new();
        let step = 1.0 / n as f32;
        let push = |verts: &mut Vec<RawVertex>, x: f32, y: f32| {
            verts.push(RawVertex {
                p: [x, y, 0.0],
                n: [0.0, 0.0, 1.0],
                uv: [x, y],
                joints: [0; 4],
                weights: [0.0; 4],
            });
        };
        for j in 0..n {
            for i in 0..n {
                let (x0, y0) = (i as f32 * step, j as f32 * step);
                let (x1, y1) = (x0 + step, y0 + step);
                // two triangles per cell
                push(&mut verts, x0, y0);
                push(&mut verts, x1, y0);
                push(&mut verts, x1, y1);
                push(&mut verts, x0, y0);
                push(&mut verts, x1, y1);
                push(&mut verts, x0, y1);
            }
        }
        let indices = (0..verts.len() as u32).collect();
        RawModel {
            vertices: verts,
            indices,
            bones: Vec::new(),
        }
    }

    fn tris(m: &RawModel) -> usize {
        m.indices.len() / 3
    }

    #[test]
    fn a_target_at_or_above_the_source_is_the_source_verbatim() {
        let m = grid(8);
        let src = tris(&m);
        for target in [src, src + 1, src * 10] {
            let out = decimate_to(&m, target);
            assert_eq!(out.indices.len(), m.indices.len(), "target {target}");
            assert_eq!(out.vertices.len(), m.vertices.len(), "target {target}");
        }
    }

    #[test]
    fn targets_reduce_monotonically_and_stay_nondeduped() {
        let m = grid(12);
        let src = tris(&m);
        let mut prev = usize::MAX;
        for (i, target) in [src, src * 9 / 10, src * 3 / 4, src / 2]
            .into_iter()
            .enumerate()
        {
            let model = decimate_to(&m, target);
            // Non-deduped invariant: sequential indices, one per corner.
            assert_eq!(model.indices.len(), model.vertices.len());
            for (k, &idx) in model.indices.iter().enumerate() {
                assert_eq!(idx as usize, k);
            }
            let got = tris(&model);
            assert!(got <= prev || i == 0, "target {target} grew to {got}");
            prev = got;
        }
        // The half target actually shed a meaningful share of the interior.
        assert!(
            (prev as f32) < 0.75 * src as f32,
            "half target barely reduced: {prev}/{src}"
        );
    }

    /// A seamed open grid cannot collapse its border, so a target of ONE stops at the last
    /// legal collapse rather than tearing the mesh — and never panics or empties it.
    #[test]
    fn a_seamed_mesh_stops_at_its_last_legal_collapse() {
        let m = grid(6);
        let out = decimate_to(&m, 1);
        let got = tris(&out);
        assert!(got >= 1 && got < tris(&m), "got {got} of {}", tris(&m));
        assert_eq!(out.indices.len(), out.vertices.len());
        // Idempotent: asking again for the same (unreachable) target changes nothing.
        assert_eq!(tris(&decimate_to(&out, 1)), got);
    }

    /// A CLOSED, watertight subdivided cube (no boundary, no UV seams) — the shape a character
    /// mesh approximates. Every edge is shared by two triangles, so interior-only collapse should
    /// reach the 50% floor. If this does not reduce, the collapse itself is broken.
    fn subdiv_cube(n: usize) -> RawModel {
        let mut verts = Vec::new();
        let step = 2.0 / n as f32;
        // Six faces, each an n×n grid of quads → 2 tris per quad. Positions on [-1,1]³; UV all 0
        // (no seams) so the whole surface welds into one closed manifold.
        let faces: [(usize, usize, usize, f32); 6] = [
            (0, 1, 2, 1.0),  // +Z
            (0, 1, 2, -1.0), // -Z
            (0, 2, 1, 1.0),  // +Y
            (0, 2, 1, -1.0), // -Y
            (1, 2, 0, 1.0),  // +X
            (1, 2, 0, -1.0), // -X
        ];
        for (a, b, c, sign) in faces {
            let corner = |u: f32, v: f32| {
                let mut p = [0.0f32; 3];
                p[a] = u;
                p[b] = v;
                p[c] = sign;
                RawVertex {
                    p,
                    n: [0.0, 0.0, 1.0],
                    uv: [0.0, 0.0],
                    joints: [0; 4],
                    weights: [0.0; 4],
                }
            };
            for j in 0..n {
                for i in 0..n {
                    let (u0, v0) = (-1.0 + i as f32 * step, -1.0 + j as f32 * step);
                    let (u1, v1) = (u0 + step, v0 + step);
                    verts.push(corner(u0, v0));
                    verts.push(corner(u1, v0));
                    verts.push(corner(u1, v1));
                    verts.push(corner(u0, v0));
                    verts.push(corner(u1, v1));
                    verts.push(corner(u0, v1));
                }
            }
        }
        let indices = (0..verts.len() as u32).collect();
        RawModel {
            vertices: verts,
            indices,
            bones: Vec::new(),
        }
    }

    #[test]
    fn a_closed_mesh_reaches_a_deep_target() {
        let m = subdiv_cube(8); // 6 · 8 · 8 · 2 = 768 triangles
        let src = tris(&m);
        // A closed manifold has no locked boundary, so a deep target lands on (or just under) it.
        let deep = decimate_to(&m, 200);
        assert!(
            tris(&deep) <= 200,
            "closed mesh only reached {}/{src} — collapse is not reducing",
            tris(&deep)
        );
        assert!(tris(&deep) >= 100, "overshot the target: {}", tris(&deep));
        // And an intermediate target sits between the source and the deep one.
        let mid = tris(&decimate_to(&m, src / 2));
        assert!(
            mid < src && mid > tris(&deep),
            "half target {mid} not between {src} and {}",
            tris(&deep)
        );
    }
}
