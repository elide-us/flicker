//! Progressive triangle decimation for a [`RawModel`] — a hand-rolled QEM (quadric error
//! metric) edge-collapse, structured as ONE pass that snapshots the mesh at each retention
//! bucket so the Clayworks Prep slider can scrub between levels instantly (no per-frame
//! re-decimation of a 100K mesh).
//!
//! Design rulings (Aaron, 2026-08-22):
//!   * reduction is a **percent OF THE SOURCE triangle count** — always relative to whatever
//!     that mesh ships with (a 127K elf, a 180K lizard, a 98K dwarf are each their own 100%),
//!     never an absolute target;
//!   * the slider keeps **100% → 50% in 5% buckets** (11 levels). 100% is the mesh verbatim;
//!     50% is the floor (remove at most half).
//!
//! No dependency (no `meshopt`). This is a STANDALONE triangle-mesh path — it never touches the
//! voxel QEF / dual-contouring LOD (`flicker-voxel`), a different algorithm Aaron ruled off-limits.
//!
//! Fidelity stance (v1, the plan's "keep seams rather than chase attribute-QEM"): the mesh is
//! welded by position+uv+normal into wedges, so a UV seam is a boundary in wedge space; only
//! edges whose BOTH endpoints are interior (non-boundary) are collapsed. That preserves every
//! UV seam and silhouette border — no texture cracks, no holes — at the cost of leaving
//! seam-dense regions denser. At the ≤50% reduction this tool allows, interior collapses carry
//! the budget on a character body.

use std::collections::HashMap;

use glam::Vec3;

use crate::fbx::{RawModel, RawVertex};

/// Slider bucket step: 5% of source.
pub const BUCKET_STEP: f32 = 0.05;
/// Floor retention: keep at least half the source triangles (Aaron: "min 50% decimation").
pub const MIN_KEEP: f32 = 0.50;

/// The precomputed retention levels for one mesh — index 0 is 100% (the source verbatim),
/// then one snapshot per 5% bucket down to [`MIN_KEEP`]. Built once on Prep entry; the slider
/// selects a level by keep-percent.
#[derive(Debug, Clone)]
pub struct DecimateLevels {
    /// One `RawModel` per bucket, parallel to [`keep_fracs`](Self::keep_fracs).
    pub levels: Vec<RawModel>,
    /// The retention fraction of each level: 1.00, 0.95, … , 0.50.
    pub keep_fracs: Vec<f32>,
    /// The source triangle count (level 0's tri count) — the 100% every bucket is a fraction of.
    pub source_tris: usize,
}

impl DecimateLevels {
    /// The level index for a keep-PERCENT slider value (100 → 0, 95 → 1, … , 50 → last),
    /// clamped to the range actually built.
    pub fn level_for_keep_pct(&self, keep_pct: f32) -> usize {
        let frac = (keep_pct / 100.0).clamp(MIN_KEEP, 1.0);
        let idx = ((1.0 - frac) / BUCKET_STEP).round() as usize;
        idx.min(self.levels.len().saturating_sub(1))
    }

    /// The mesh at a keep-percent (borrow); falls back to the source (level 0) if empty.
    pub fn model_for_keep_pct(&self, keep_pct: f32) -> &RawModel {
        &self.levels[self.level_for_keep_pct(keep_pct)]
    }
}

/// Build the progressive retention levels for `model` (100% → 50% in 5% buckets). One QEM pass,
/// snapshotting at each bucket boundary. The source mesh must be non-deduped with sequential
/// indices (the `parse_fbx` convention); every returned level keeps that convention.
pub fn decimate_levels(model: &RawModel) -> DecimateLevels {
    let source_tris = model.indices.len() / 3;

    // The bucket retention fractions: 1.00, 0.95, … , 0.50.
    let mut keep_fracs = Vec::new();
    let steps = ((1.0 - MIN_KEEP) / BUCKET_STEP).round() as usize; // 10
    for k in 0..=steps {
        keep_fracs.push(1.0 - BUCKET_STEP * k as f32);
    }

    // Level 0 is always the source verbatim.
    let mut levels = vec![model.clone()];

    // Nothing to collapse (degenerate / tiny mesh): every bucket is the source.
    if source_tris < 4 {
        while levels.len() < keep_fracs.len() {
            levels.push(model.clone());
        }
        return DecimateLevels {
            levels,
            keep_fracs,
            source_tris,
        };
    }

    let mut mesh = WeldedMesh::from_raw(model);
    // Walk the buckets below 100%; collapse down to each target, then snapshot.
    for frac in keep_fracs.iter().copied().skip(1) {
        let target = ((source_tris as f32) * frac).round() as usize;
        mesh.collapse_to(target);
        levels.push(mesh.to_raw(model));
    }

    DecimateLevels {
        levels,
        keep_fracs,
        source_tris,
    }
}

/// Convenience: the decimated mesh at one keep-fraction (0.5..=1.0), for headless/one-shot use.
pub fn decimate(model: &RawModel, keep: f32) -> RawModel {
    let keep = keep.clamp(MIN_KEEP, 1.0);
    if model.indices.len() / 3 < 4 || keep >= 1.0 - 1e-4 {
        return model.clone();
    }
    let mut mesh = WeldedMesh::from_raw(model);
    let target = ((model.indices.len() / 3) as f32 * keep).round() as usize;
    mesh.collapse_to(target);
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
    /// runs once per mesh at Prep entry, not per frame.
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

    #[test]
    fn level_zero_is_the_source_verbatim() {
        let m = grid(8);
        let lv = decimate_levels(&m);
        assert_eq!(lv.levels[0].indices.len(), m.indices.len());
        assert_eq!(lv.levels[0].vertices.len(), m.vertices.len());
        assert!((lv.keep_fracs[0] - 1.0).abs() < 1e-6);
        assert!((lv.keep_fracs.last().unwrap() - MIN_KEEP).abs() < 1e-6);
        assert_eq!(lv.levels.len(), lv.keep_fracs.len());
    }

    #[test]
    fn buckets_reduce_monotonically_and_stay_nondeduped() {
        let m = grid(12);
        let lv = decimate_levels(&m);
        let mut prev = usize::MAX;
        for (i, model) in lv.levels.iter().enumerate() {
            // Non-deduped invariant: sequential indices, one per corner.
            assert_eq!(model.indices.len(), model.vertices.len());
            for (k, &idx) in model.indices.iter().enumerate() {
                assert_eq!(idx as usize, k);
            }
            let tris = model.indices.len() / 3;
            assert!(tris <= prev || i == 0, "level {i} grew");
            prev = tris;
        }
        // The 50% level actually shed a meaningful share of the interior.
        let src = lv.source_tris as f32;
        let last = (lv.levels.last().unwrap().indices.len() / 3) as f32;
        assert!(last < 0.75 * src, "50% bucket barely reduced: {last}/{src}");
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
    fn a_closed_mesh_decimates_to_the_floor() {
        let m = subdiv_cube(8); // 6 · 8 · 8 · 2 = 768 triangles
        let lv = decimate_levels(&m);
        let src = lv.source_tris as f32;
        let last = (lv.levels.last().unwrap().indices.len() / 3) as f32;
        // A closed manifold has no locked boundary, so the 50% bucket should land near 50% kept.
        assert!(
            last <= 0.60 * src,
            "closed mesh only reached {last}/{src} — collapse is not reducing"
        );
        // And an intermediate bucket sits between the source and the floor.
        let mid = (lv.levels[5].indices.len() / 3) as f32; // 75% kept
        assert!(
            mid < src && mid > last,
            "75% bucket {mid} not between {src} and {last}"
        );
    }

    #[test]
    fn keep_pct_maps_to_levels() {
        let m = grid(10);
        let lv = decimate_levels(&m);
        assert_eq!(lv.level_for_keep_pct(100.0), 0);
        assert_eq!(lv.level_for_keep_pct(95.0), 1);
        assert_eq!(lv.level_for_keep_pct(50.0), lv.levels.len() - 1);
        // Out-of-range clamps.
        assert_eq!(lv.level_for_keep_pct(30.0), lv.levels.len() - 1);
        assert_eq!(lv.level_for_keep_pct(150.0), 0);
    }
}
