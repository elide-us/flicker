//! MESH MIRROR — the half-body mirror verb (direction 697DEC55, third of the ratified tool order
//! 42AB9BA8), the source-shape fix for a LOPSIDED sculpt: the elf that came back with two thigh
//! lengths and its whole body offset left (2D31782B). One mirror and every symmetric-rig
//! assumption holds again — the fit's twin joints, [`square_stance`](crate::bake::square_stance)'s
//! reflection, the bench's MIRROR →.
//!
//! It runs in **PREP**, before the skeleton is fitted, because it changes the SOURCE SHAPE: cut the
//! mesh on the median plane, drop the half you do not want, reflect the half you do. What a
//! half-body mirror DESTROYS is everything one-sided — a mane that falls to one side doubles or
//! vanishes, a tail across the plane truncates — so the TAGGED regions are carried through as
//! authored instead, which is exactly what the region tagger (0A7793F8) put
//! [`RawModel::regions`] there for.
//!
//! The side to KEEP is the author's (ruling FEFDA2B2: *"there's no guarantee it will be one
//! specific side"*) — the same choice the stance normaliser takes.

use std::collections::HashMap;

use glam::{Mat4, Vec3};
use serde::{Deserialize, Serialize};

use crate::bake::{cell_key, side_of, twin_name};
use crate::fbx::{RawModel, RawVertex};

/// How near X = 0 a vertex counts as already ON the median plane — inside this it is SHARED by the
/// two halves rather than cut and reflected, which is what makes the seam weld exact.
const ON_PLANE_CM: f32 = 1e-3;
/// How far a kept region's vertex may sit from the NEW body surface and still be welded onto it:
/// one Flesh cell (3995EF9E). Inside a cell it is the region's attachment seam; outside it is the
/// region's own fall and stays where it was authored.
const WELD_CM: f32 = 1.0;

/// Which half of the body the mirror KEEPS. The body's LEFT is +X (the canon puts `eye_l` there,
/// facing −Y), so `Left` keeps `x ≥ 0` and reflects it onto the right.
///
/// The bench's Prep control is "Mirror from: Off · Left · Right" — Off is simply not calling
/// [`mirror_mesh`], the way `Auto` is a [`StanceSource`](crate::bake::StanceSource) and not a
/// fourth mirror side: a mirror has no pose to read a side off.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Side {
    Left,
    Right,
}

impl Side {
    /// The sign of X this side lives on.
    fn sign(self) -> f32 {
        match self {
            Side::Left => 1.0,
            Side::Right => -1.0,
        }
    }
}

/// What [`mirror_mesh`] did: source vertices that went with the discarded half, vertices added (the
/// reflected half plus the cut boundary), and how many tagged regions came through.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MirrorReport {
    pub dropped: usize,
    pub added: usize,
    pub kept_regions: usize,
}

/// MIRROR THE MESH across X = 0, keeping `keep`.
///
/// 1. every triangle crossing the plane is CUT on it exactly — the new vertices carry an
///    interpolated normal, UV and skin, and their X is set to literal zero;
/// 2. the discarded half's vertices and triangles are dropped, EXCEPT anything belonging to a
///    region in [`RawModel::regions`] — a kept mane stays as authored, on whichever side it hangs;
/// 3. the kept half's BODY triangles are reflected: `x → −x`, the normal's X negated, the winding
///    flipped, the UV copied, the skin moved onto each bone's `_l`/`_r` twin where one exists;
/// 4. the plane's vertices are WELDED — the cut boundary coincides with its own reflection, so the
///    two halves share it and the mesh closes — and a kept region's seam (anything within
///    [`WELD_CM`] of the new surface) is snapped onto the body it now hangs from;
/// 5. with a skeleton present, every bone of the discarded side goes to its twin's reflection (the
///    reflection [`square_stance`](crate::bake::square_stance) puts a raised limb at) and the body
///    re-binds through the conform's own writer.
///
/// A mesh that does not straddle the plane has no half to mirror — it is warned about and left
/// exactly as it was, rather than silently doubled into a pair of shells.
pub fn mirror_mesh(model: &mut RawModel, keep: Side) -> MirrorReport {
    let s = keep.sign();
    // KEEP (+1) · the median plane (0) · DISCARD (−1), per source vertex.
    let half: Vec<i8> = model
        .vertices
        .iter()
        .map(|v| {
            let x = v.p[0] * s;
            if x > ON_PLANE_CM {
                1
            } else if x < -ON_PLANE_CM {
                -1
            } else {
                0
            }
        })
        .collect();
    if !half.contains(&1) || !half.contains(&-1) {
        tracing::warn!(
            "mirror_mesh: the mesh does not straddle X = 0 ({} on the {keep:?} side, {} on the other) — nothing to mirror",
            half.iter().filter(|&&h| h == 1).count(),
            half.iter().filter(|&&h| h == -1).count(),
        );
        return MirrorReport::default();
    }

    // Region membership is per-VERTEX and the mesh is one vertex per CORNER, so a tagged position
    // is tagged wherever it appears — welded by position, the ONE convention (1B64FF03). A
    // triangle whose corners are all tagged is a region's own; everything else is body.
    let (corner, welds) = crate::fbx::weld_by_position(&model.vertices);
    let mut tagged = vec![false; welds.len()];
    for r in &model.regions {
        for &v in &r.verts {
            if let Some(&c) = corner.get(v as usize) {
                tagged[c as usize] = true;
            }
        }
    }
    let protected: Vec<bool> = corner.iter().map(|&c| tagged[c as usize]).collect();

    // Each bone's `_l`/`_r` twin (itself when it has none) — the map the reflected skin rides.
    let by_name: HashMap<&str, u32> = model
        .bones
        .iter()
        .enumerate()
        .map(|(i, b)| (b.name.as_str(), i as u32))
        .collect();
    let twin: Vec<u32> = model
        .bones
        .iter()
        .enumerate()
        .map(|(i, b)| {
            twin_name(&b.name)
                .and_then(|t| by_name.get(&*t).copied())
                .unwrap_or(i as u32)
        })
        .collect();

    let mut verts: Vec<RawVertex> = Vec::with_capacity(model.vertices.len());
    let mut indices: Vec<u32> = Vec::with_capacity(model.indices.len());
    // Source vertex → its place in the rebuilt mesh, as authored and as reflected.
    let mut kept_of = vec![u32::MAX; model.vertices.len()];
    let mut mirror_of = vec![u32::MAX; model.vertices.len()];
    let mut seam_ids: Vec<u32> = Vec::new();

    for t in model.indices.as_chunks::<3>().0 {
        let tri = [t[0] as usize, t[1] as usize, t[2] as usize];
        if tri.iter().any(|&i| i >= model.vertices.len()) {
            continue;
        }
        if tri.iter().all(|&i| protected[i]) {
            // A TAGGED region's own triangle passes through as authored, on either side of the
            // plane and never reflected: one mane stays one mane.
            for &i in &tri {
                let id = emit(&mut verts, &mut kept_of, i, || model.vertices[i]);
                indices.push(id);
            }
            continue;
        }
        let sides = [half[tri[0]], half[tri[1]], half[tri[2]]];
        if !sides.contains(&1) {
            // Wholly discarded — or lying IN the plane, where it is its own reflection and the
            // kept half's own surface already carries it.
            continue;
        }
        // Clip to the kept half (Sutherland–Hodgman over the ring), emitting each surviving corner
        // twice: as authored, and reflected. A corner ON the plane is ONE vertex serving both —
        // that shared vertex IS the weld.
        let mut ids: Vec<u32> = Vec::with_capacity(4);
        let mut mids: Vec<u32> = Vec::with_capacity(4);
        for (k, &here) in sides.iter().enumerate() {
            let (i, j) = (tri[k], tri[(k + 1) % 3]);
            if here >= 0 {
                let on_plane = here == 0;
                let id = emit(&mut verts, &mut kept_of, i, || {
                    let v = model.vertices[i];
                    if on_plane {
                        seam(v)
                    } else {
                        v
                    }
                });
                ids.push(id);
                mids.push(if on_plane {
                    id
                } else {
                    emit(&mut verts, &mut mirror_of, i, || {
                        reflect(&model.vertices[i], &twin)
                    })
                });
            }
            if here * sides[(k + 1) % 3] < 0 {
                let id = verts.len() as u32;
                verts.push(cut(&model.vertices[i], &model.vertices[j]));
                seam_ids.push(id);
                ids.push(id);
                mids.push(id);
            }
        }
        for k in 1..ids.len().saturating_sub(1) {
            indices.extend([ids[0], ids[k], ids[k + 1]]);
            // A reflection reverses handedness — the winding turns with it, or the mirrored half
            // faces inward.
            indices.extend([mids[0], mids[k + 1], mids[k]]);
        }
    }

    let carried = kept_of.iter().filter(|&&x| x != u32::MAX).count();
    let mut report = MirrorReport {
        dropped: model.vertices.len() - carried,
        added: verts.len() - carried,
        kept_regions: 0,
    };

    // The regions ride the rebuild: membership (and any bind already derived from it) follows the
    // vertices that survived, and a region the mirror ate entirely drops out.
    for r in &mut model.regions {
        let at = |v: u32| kept_of.get(v as usize).copied().unwrap_or(u32::MAX);
        r.verts.retain_mut(|v| {
            *v = at(*v);
            *v != u32::MAX
        });
        r.binds.retain_mut(|b| {
            b.v = at(b.v);
            b.v != u32::MAX
        });
        report.kept_regions += usize::from(!r.verts.is_empty());
    }

    // WELD a kept region's SEAM onto the new body: the half a mane hung off has just been replaced
    // by the reflection of the other, so everything of it within a cell of the new skin is snapped
    // onto that skin. Further out is the region's own fall and stays exactly as authored. Only the
    // DISCARDED side needs it — on the kept side the body never moved.
    if report.kept_regions > 0 {
        let mut surface: Vec<u32> = seam_ids;
        surface.extend(mirror_of.iter().copied().filter(|&x| x != u32::MAX));
        for (i, &id) in kept_of.iter().enumerate() {
            if id != u32::MAX && !protected[i] {
                surface.push(id);
            }
        }
        let mut grid: HashMap<(i32, i32, i32), Vec<u32>> = HashMap::new();
        for &id in &surface {
            let key = cell_key(Vec3::from_array(verts[id as usize].p), WELD_CM);
            grid.entry(key).or_default().push(id);
        }
        for i in 0..model.vertices.len() {
            if !protected[i] || half[i] >= 0 || kept_of[i] == u32::MAX {
                continue;
            }
            let p = Vec3::from_array(verts[kept_of[i] as usize].p);
            let c = cell_key(p, WELD_CM);
            let mut best: Option<(f32, u32)> = None;
            for dx in -1..=1 {
                for dy in -1..=1 {
                    for dz in -1..=1 {
                        let cell = grid.get(&(c.0 + dx, c.1 + dy, c.2 + dz));
                        for &j in cell.into_iter().flatten() {
                            let d = p.distance(Vec3::from_array(verts[j as usize].p));
                            if d <= WELD_CM && best.is_none_or(|(bd, _)| d < bd) {
                                best = Some((d, j));
                            }
                        }
                    }
                }
            }
            if let Some((_, j)) = best {
                let onto = verts[j as usize].p;
                verts[kept_of[i] as usize].p = onto;
            }
        }
    }

    // THE SKELETON follows its mesh: every bone of the discarded side lands at its twin's
    // reflection, and the body re-binds through the conform's own writer — the same two moves
    // `square_stance` makes, for the same reason.
    if !model.bones.is_empty() {
        let world = crate::conform::model_world_frames(model);
        let moved: Vec<Mat4> = world
            .iter()
            .enumerate()
            .map(|(i, g)| {
                if side_of(&model.bones[i].name) != Some(keep == Side::Right) {
                    return *g;
                }
                let t = twin[i] as usize;
                if t == i {
                    tracing::warn!(
                        "mirror_mesh: {} has no twin to be reflected from — left as placed",
                        model.bones[i].name
                    );
                    return *g;
                }
                let p = world[t].w_axis.truncate();
                Mat4::from_cols(
                    g.x_axis,
                    g.y_axis,
                    g.z_axis,
                    Vec3::new(-p.x, p.y, p.z).extend(1.0),
                )
            })
            .collect();
        crate::conform::write_world_frames(&mut model.bones, &moved);
    }

    model.vertices = verts;
    model.indices = indices;
    tracing::info!(
        "mirror_mesh: kept the {keep:?} half — {} vertices dropped, {} added, {} region(s) carried",
        report.dropped,
        report.added,
        report.kept_regions
    );
    report
}

/// Place source vertex `i` in the rebuilt mesh ONCE under `map`, building it with `make` the first
/// time. Both the as-authored and the reflected copy are addressed this way, one map each.
fn emit(
    out: &mut Vec<RawVertex>,
    map: &mut [u32],
    i: usize,
    make: impl FnOnce() -> RawVertex,
) -> u32 {
    if map[i] == u32::MAX {
        map[i] = out.len() as u32;
        out.push(make());
    }
    map[i]
}

/// A vertex ON the plane, shared by both halves: X pinned to literal zero so the weld is exact, and
/// its normal the mean of itself and its own reflection — whose X cancels.
fn seam(mut v: RawVertex) -> RawVertex {
    v.p[0] = 0.0;
    let n = Vec3::new(0.0, v.n[1], v.n[2]);
    if n.length_squared() > 1e-12 {
        v.n = n.normalize().to_array();
    }
    v
}

/// The REFLECTION of a vertex across X = 0: the normal's X negated with it, the UV as authored, and
/// the skin moved onto each bone's `_l`/`_r` twin — a bone without one keeps its own influence.
fn reflect(v: &RawVertex, twin: &[u32]) -> RawVertex {
    RawVertex {
        p: [-v.p[0], v.p[1], v.p[2]],
        n: [-v.n[0], v.n[1], v.n[2]],
        joints: v.joints.map(|j| twin.get(j as usize).copied().unwrap_or(j)),
        ..*v
    }
}

/// The vertex where the edge `a`–`b` crosses X = 0 (the caller has established that it does), with
/// its normal, UV and skin interpolated — then pinned to the plane by [`seam`].
fn cut(a: &RawVertex, b: &RawVertex) -> RawVertex {
    let t = a.p[0] / (a.p[0] - b.p[0]);
    let lerp = |x: f32, y: f32| x + (y - x) * t;
    let (joints, weights) = blend_skin(a, b, t);
    seam(RawVertex {
        p: [0.0, lerp(a.p[1], b.p[1]), lerp(a.p[2], b.p[2])],
        n: Vec3::new(
            lerp(a.n[0], b.n[0]),
            lerp(a.n[1], b.n[1]),
            lerp(a.n[2], b.n[2]),
        )
        .normalize_or_zero()
        .to_array(),
        uv: [lerp(a.uv[0], b.uv[0]), lerp(a.uv[1], b.uv[1])],
        joints,
        weights,
    })
}

/// Two vertices' 4-influence skins blended `t` of the way from `a` to `b`: per bone, then the top
/// four renormalised — the same shape [`crate::bake::bake_skin`] emits.
fn blend_skin(a: &RawVertex, b: &RawVertex, t: f32) -> ([u32; 4], [f32; 4]) {
    let mut acc: Vec<(u32, f32)> = Vec::with_capacity(8);
    for (v, share) in [(a, 1.0 - t), (b, t)] {
        for k in 0..4 {
            let w = v.weights[k] * share;
            if w <= 0.0 {
                continue;
            }
            match acc.iter_mut().find(|(j, _)| *j == v.joints[k]) {
                Some(e) => e.1 += w,
                None => acc.push((v.joints[k], w)),
            }
        }
    }
    acc.sort_by(|p, q| q.1.total_cmp(&p.1));
    acc.truncate(4);
    let sum: f32 = acc.iter().map(|(_, w)| w).sum();
    let (mut joints, mut weights) = ([0u32; 4], [0.0f32; 4]);
    for (k, &(j, w)) in acc.iter().enumerate() {
        joints[k] = j;
        weights[k] = if sum > 0.0 { w / sum } else { 0.0 };
    }
    (joints, weights)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fbx::{RawBone, RawVertex};
    use flicker_skeletal::format::{ClothParams, ClothRegion, RegionTag};
    use std::collections::HashMap;

    /// A corner vertex: UV carries X too, so "the UV is COPIED, not mirrored" is testable — a
    /// reflected vertex must wear its SOURCE's UV, not the one its new position would imply.
    fn vert(p: Vec3, n: Vec3, joint: u32) -> RawVertex {
        RawVertex {
            p: p.to_array(),
            n: n.to_array(),
            uv: [(p.x + 20.0) / 40.0, p.z / 30.0],
            joints: [joint, 0, 0, 0],
            weights: [1.0, 0.0, 0.0, 0.0],
        }
    }

    /// The 12 triangles of a box, outward-wound, as per-corner vertices (the `parse_fbx`
    /// convention). Corner `i` is `x + 2y + 4z` over (lo, hi) per axis.
    fn box_tris(lo: Vec3, hi: Vec3, joint_of: impl Fn(Vec3) -> u32, out: &mut RawModel) {
        let c = |i: usize| {
            Vec3::new(
                if i & 1 == 0 { lo.x } else { hi.x },
                if i & 2 == 0 { lo.y } else { hi.y },
                if i & 4 == 0 { lo.z } else { hi.z },
            )
        };
        const FACES: [[usize; 3]; 12] = [
            [0, 4, 6],
            [0, 6, 2],
            [1, 3, 7],
            [1, 7, 5],
            [0, 1, 5],
            [0, 5, 4],
            [2, 6, 7],
            [2, 7, 3],
            [0, 2, 3],
            [0, 3, 1],
            [4, 5, 7],
            [4, 7, 6],
        ];
        for f in FACES {
            let (a, b, d) = (c(f[0]), c(f[1]), c(f[2]));
            let n = (b - a).cross(d - a).normalize();
            for p in [a, b, d] {
                out.indices.push(out.vertices.len() as u32);
                out.vertices.push(vert(p, n, joint_of(p)));
            }
        }
    }

    /// A LOPSIDED body (2D31782B, the elf): a box running −10..+20 in X, so its LEFT half is twice
    /// the width of its right, on a pelvis with one thigh pushed out with it. Every vertex is
    /// skinned to the thigh on its own side, so the reflected skin's `_l`/`_r` swap is visible.
    fn lopsided_body() -> RawModel {
        let world = [
            ("root", -1, Vec3::new(0.0, 0.0, 0.0)),
            ("pelvis", 0, Vec3::new(0.0, 0.0, 15.0)),
            ("thigh_l", 1, Vec3::new(12.0, 0.0, 10.0)),
            ("thigh_r", 1, Vec3::new(-6.0, 0.0, 10.0)),
        ];
        let bones = world
            .iter()
            .map(|&(name, parent, w)| {
                let p = usize::try_from(parent)
                    .ok()
                    .map_or(Vec3::ZERO, |i| world[i].2);
                RawBone {
                    name: name.to_string(),
                    parent,
                    translation: (w - p).to_array(),
                    rotation: [0.0, 0.0, 0.0, 1.0],
                    scale: [1.0, 1.0, 1.0],
                    inverse_bind: Mat4::from_translation(w).inverse().to_cols_array(),
                }
            })
            .collect();
        let mut m = RawModel {
            bones,
            ..Default::default()
        };
        box_tris(
            Vec3::new(-10.0, -5.0, 0.0),
            Vec3::new(20.0, 5.0, 30.0),
            |p| if p.x > 0.0 { 2 } else { 3 },
            &mut m,
        );
        m
    }

    fn bone_at(model: &RawModel, name: &str) -> Vec3 {
        let i = model.bones.iter().position(|b| b.name == name).unwrap();
        crate::conform::model_world_frames(model)[i]
            .w_axis
            .truncate()
    }

    /// Every welded edge is shared by exactly two triangles — the mesh is closed, which is the
    /// property the plane weld exists to protect: a DUPLICATED seam would leave every seam edge
    /// on one triangle only.
    fn open_edges(model: &RawModel) -> usize {
        let (corner, _) = crate::fbx::weld_by_position(&model.vertices);
        let mut count: HashMap<(u32, u32), usize> = HashMap::new();
        for t in model.indices.as_chunks::<3>().0 {
            for k in 0..3 {
                let (a, b) = (corner[t[k] as usize], corner[t[(k + 1) % 3] as usize]);
                if a == b {
                    continue; // a degenerate edge belongs to no surface
                }
                *count.entry((a.min(b), a.max(b))).or_default() += 1;
            }
        }
        count.values().filter(|&&n| n != 2).count()
    }

    /// THE MIRROR'S REAL JOB (697DEC55): a lopsided body mirrored from the NARROW side comes back
    /// symmetric — every vertex with a partner at its own reflection, wearing its SOURCE's UV, the
    /// mesh still closed across the plane, and the discarded side's bones at their twins'
    /// reflections with the skin moved onto them.
    #[test]
    fn a_lopsided_body_mirrored_from_the_narrow_side_comes_back_symmetric() {
        let mut m = lopsided_body();
        let before = m.vertices.len();
        let report = mirror_mesh(&mut m, Side::Right);
        assert!(
            report.dropped > 0 && report.added > 0,
            "a half was replaced: {report:?}"
        );
        assert_eq!(report.kept_regions, 0, "an untagged body has no regions");

        // The wide half is GONE: the body now runs −10..+10, its narrow side's reflection.
        let (lo, hi) = m.vertices.iter().fold((f32::MAX, f32::MIN), |(l, h), v| {
            (l.min(v.p[0]), h.max(v.p[0]))
        });
        assert!(
            (lo + 10.0).abs() < 1e-3 && (hi - 10.0).abs() < 1e-3,
            "the body spans −10..+10, got {lo}..{hi}"
        );

        // SYMMETRIC: every vertex has a partner at its reflection, carrying its own UV across.
        let by_pos: HashMap<[i64; 3], usize> = m
            .vertices
            .iter()
            .enumerate()
            .map(|(i, v)| {
                let q = |f: f32| (f as f64 * 1000.0).round() as i64;
                ([q(v.p[0]), q(v.p[1]), q(v.p[2])], i)
            })
            .collect();
        for v in &m.vertices {
            let q = |f: f32| (f as f64 * 1000.0).round() as i64;
            let key = [q(-v.p[0]), q(v.p[1]), q(v.p[2])];
            let Some(&j) = by_pos.get(&key) else {
                panic!("{:?} has no mirror partner", v.p);
            };
            let p = Vec3::from_array(m.vertices[j].p);
            assert!(
                p.distance(Vec3::new(-v.p[0], v.p[1], v.p[2])) < 1e-3,
                "{:?} vs its partner {p}",
                v.p
            );
        }
        // The UV is COPIED: a reflected vertex wears the UV of the vertex it was reflected FROM,
        // which on this fixture is not the one its new position would give it.
        for v in m.vertices.iter().filter(|v| v.p[0] > 1e-3) {
            let source_u = (-v.p[0] + 20.0) / 40.0;
            assert!(
                (v.uv[0] - source_u).abs() < 1e-4,
                "the mirrored vertex at {:?} keeps its source's UV {source_u}, got {}",
                v.p,
                v.uv[0]
            );
        }

        // WATERTIGHT across the plane, and a consistent index buffer.
        assert_eq!(open_edges(&m), 0, "the mirrored body is closed");
        assert_eq!(m.indices.len() % 3, 0, "whole triangles");
        assert!(m.indices.iter().all(|&i| (i as usize) < m.vertices.len()));

        // WINDING: the box is convex about its centre, so every face must look away from it.
        let centre = Vec3::new(0.0, 0.0, 15.0);
        for t in m.indices.as_chunks::<3>().0 {
            let p: Vec<Vec3> = t
                .iter()
                .map(|&i| Vec3::from_array(m.vertices[i as usize].p))
                .collect();
            let n = (p[1] - p[0]).cross(p[2] - p[0]);
            assert!(
                n.dot(p[0] - centre) > 0.0,
                "the face at {:?} is wound outward",
                p[0]
            );
        }

        // THE SKELETON followed: the discarded side's thigh is at its twin's reflection…
        assert!(
            bone_at(&m, "thigh_l").distance(Vec3::new(6.0, 0.0, 10.0)) < 1e-3,
            "thigh_l lands at thigh_r's reflection, got {}",
            bone_at(&m, "thigh_l")
        );
        assert!(
            bone_at(&m, "thigh_r").distance(Vec3::new(-6.0, 0.0, 10.0)) < 1e-6,
            "the kept side's thigh never moved"
        );
        // …and the reflected skin rides the TWIN bone, not the one it was authored on.
        let (l, r) = (2u32, 3u32);
        for v in &m.vertices {
            let want = if v.p[0] > 1e-3 { l } else { r };
            if v.p[0].abs() > 1e-3 {
                assert_eq!(
                    v.joints[0], want,
                    "the vertex at {:?} is skinned to its own side's thigh",
                    v.p
                );
            }
        }
        assert!(m.vertices.len() > before / 2, "a whole body came out");
    }

    /// A curved surface obliquely clipped is where a winding or a seam mistake shows: a lopsided
    /// sphere (an ODD longitude count, so triangles genuinely straddle the plane) comes back
    /// closed, outward-wound and symmetric.
    #[test]
    fn a_mirrored_sphere_stays_closed_and_wound_outward() {
        let (rings, segs) = (6usize, 7usize);
        let at = |i: usize, j: usize| {
            let theta = std::f32::consts::PI * i as f32 / rings as f32;
            let phi = std::f32::consts::TAU * j as f32 / segs as f32;
            // The poles are written EXACTLY, so the cap's slivers are exactly degenerate and drop
            // out below — `PI.sin()` is 1e-7, not 0, and a 1e-7-wide sliver is not a surface.
            let p = match i {
                0 => Vec3::Z,
                _ if i == rings => Vec3::NEG_Z,
                _ => Vec3::new(
                    theta.sin() * phi.cos(),
                    theta.sin() * phi.sin(),
                    theta.cos(),
                ),
            } * 20.0;
            // LOPSIDED, the sculpt this verb is for: one half stretched.
            if p.x > 0.0 {
                Vec3::new(p.x * 1.6, p.y, p.z)
            } else {
                p
            }
        };
        let mut m = RawModel::default();
        for i in 0..rings {
            for j in 0..segs {
                let (a, b, c, d) = (at(i, j), at(i, j + 1), at(i + 1, j + 1), at(i + 1, j));
                for t in [[a, d, b], [b, d, c]] {
                    if (t[1] - t[0]).cross(t[2] - t[0]).length() <= 1e-6 {
                        continue; // the degenerate sliver at a pole
                    }
                    for p in t {
                        m.indices.push(m.vertices.len() as u32);
                        m.vertices.push(vert(p, p.normalize(), 0));
                    }
                }
            }
        }
        assert_eq!(
            open_edges(&m),
            0,
            "the source sphere is closed to begin with"
        );

        let report = mirror_mesh(&mut m, Side::Right);
        assert!(report.dropped > 0 && report.added > 0, "{report:?}");
        assert_eq!(open_edges(&m), 0, "the mirrored sphere is still closed");
        for t in m.indices.as_chunks::<3>().0 {
            let p: Vec<Vec3> = t
                .iter()
                .map(|&i| Vec3::from_array(m.vertices[i as usize].p))
                .collect();
            let n = (p[1] - p[0]).cross(p[2] - p[0]);
            let centroid = (p[0] + p[1] + p[2]) / 3.0;
            assert!(
                n.dot(centroid) > 0.0,
                "the face at {centroid} is wound outward"
            );
        }
        let (lo, hi) = m.vertices.iter().fold((f32::MAX, f32::MIN), |(l, h), v| {
            (l.min(v.p[0]), h.max(v.p[0]))
        });
        // Symmetric about the plane, and the 1.6× stretch (which reached 28.8) is gone with the
        // half that carried it — both sides are now the kept half's own 18.02.
        assert!(
            (lo + hi).abs() < 1e-3 && (17.0..19.0).contains(&hi),
            "the sphere is symmetric about the plane, {lo}..{hi}"
        );
    }

    /// A TAGGED REGION SURVIVES THE DISCARDED SIDE (the direction's whole caveat, 697DEC55): a
    /// mane that falls to the half being thrown away is carried through as authored — not
    /// doubled, not cut — and its seam is welded onto the body that has just replaced the one it
    /// hung from.
    #[test]
    fn a_tagged_mane_on_the_discarded_side_is_carried_through_and_welded() {
        let mut m = lopsided_body();
        let body_verts = m.vertices.len();
        // A strip on the LEFT (+X, the discarded half): its base 4 mm off the corner the mirror
        // will put there, its fall well clear of the body.
        let strip = [
            Vec3::new(9.6, 5.0, 30.0),
            Vec3::new(9.6, -5.0, 30.0),
            Vec3::new(16.0, -5.0, 34.0),
            Vec3::new(16.0, 5.0, 34.0),
        ];
        for t in [[0, 1, 2], [0, 2, 3]] {
            for k in t {
                m.indices.push(m.vertices.len() as u32);
                m.vertices.push(vert(strip[k], Vec3::Z, 2));
            }
        }
        let mane: Vec<u32> = (body_verts as u32..m.vertices.len() as u32).collect();
        m.regions.push(ClothRegion {
            name: "mane".into(),
            anchor_bone: "pelvis".into(),
            tag: RegionTag::Mane,
            verts: mane,
            chain_count: 1,
            params: ClothParams::default(),
            chains: Vec::new(),
            binds: Vec::new(),
        });

        let report = mirror_mesh(&mut m, Side::Right);
        assert_eq!(report.kept_regions, 1, "the mane came through: {report:?}");
        assert_eq!(m.regions.len(), 1);
        let region: Vec<Vec3> = m.regions[0]
            .verts
            .iter()
            .map(|&v| Vec3::from_array(m.vertices[v as usize].p))
            .collect();
        assert_eq!(region.len(), 6, "all six corners survived, {region:?}");
        // Its FALL is exactly as authored — the mirror does not touch a region's own shape.
        for want in [strip[2], strip[3]] {
            assert!(
                region.iter().any(|p| p.distance(want) < 1e-6),
                "the mane's tip {want} is untouched"
            );
        }
        // Its SEAM welded onto the new body: the base verts snapped the 4 mm onto the corners the
        // reflection put there.
        for want in [Vec3::new(10.0, 5.0, 30.0), Vec3::new(10.0, -5.0, 30.0)] {
            assert!(
                region.iter().any(|p| p.distance(want) < 1e-4),
                "the mane's base welded onto {want}, got {region:?}"
            );
        }
        // And the mane was NOT reflected onto the kept side as well: one mane, not two.
        for ghost in [Vec3::new(-16.0, 5.0, 34.0), Vec3::new(-16.0, -5.0, 34.0)] {
            assert!(
                !m.vertices
                    .iter()
                    .any(|v| Vec3::from_array(v.p).distance(ghost) < 1e-3),
                "nothing was mirrored onto {ghost} — a tagged region is carried, never doubled"
            );
        }
    }

    /// FAIL LOUD (4BB12A75): a mesh that does not straddle the median plane has no half to mirror
    /// — mirroring it would silently produce a pair of disjoint shells, so it is reported and left
    /// exactly as it was.
    #[test]
    fn a_mesh_that_does_not_straddle_the_plane_is_left_exactly_as_it_was() {
        let mut m = RawModel::default();
        box_tris(
            Vec3::new(5.0, -5.0, 0.0),
            Vec3::new(20.0, 5.0, 30.0),
            |_| 0,
            &mut m,
        );
        let before = format!("{m:?}");
        for keep in [Side::Left, Side::Right] {
            let mut m = m.clone();
            assert_eq!(
                mirror_mesh(&mut m, keep),
                MirrorReport::default(),
                "{keep:?}: nothing to mirror"
            );
            assert_eq!(format!("{m:?}"), before, "{keep:?}: the model is untouched");
        }
    }
}
