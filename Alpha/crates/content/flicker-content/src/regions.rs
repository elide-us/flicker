//! REGIONS — a garment finds its OWN hanging panels, and each one gets the comb of jiggle chains
//! that makes it drape (spec 0A81088E, duster analysis EC30FD2E).
//!
//! A welded garment is one mesh: the collar that hugs the chest and the sleeve that swings off the
//! elbow share vertices with nothing to tell them apart. The measurement that separates them is the
//! BODY's own field — [`Flesh::distance_outside`]: a vertex farther off the body than the hang
//! threshold is cloth in flight, everything nearer (a vertex INSIDE the body included) is rigid,
//! hugging skin. The hanging set then falls into connected components over the mesh's own
//! triangles, and those components ARE the regions: they touch each other only through the rigid
//! yoke, which is exactly the seam a rigger would cut.
//!
//! [`build_cloth`] ports `tools/skin_outfit.py --build-cloth` (FA3D9851 — replicate the reference
//! mechanism): a fan of straight chains, each vertex bound to its nearest at `(chain, segment,
//! fraction)`, which [`flicker_skeletal::cloth::ClothSim`] consumes unchanged. One thing changes,
//! and it is the POC bug Aaron named: the Python hung every chain from the ANCHOR BONE, so a bell
//! sleeve's only chain started at the bone and the swing showed up at the hem alone. Here the comb
//! is laid ACROSS the region's attachment edge — `chain_count` chains, each rooted on the edge it
//! actually hangs from — so a wide panel bends along its whole width.

use std::collections::HashMap;

use flicker_skeletal::format::{ClothBind, ClothChain, ClothParams, ClothRegion, RegionTag};
use glam::{Mat3, Mat4, Vec3};

use crate::bake::rest_world_frames;
use crate::fbx::{weld_by_position, RawModel, RawVertex};
use crate::flesh::Flesh;

/// How far off the body a vertex must hang to count as cloth, in cm. 3 cm clears a fitted collar
/// and the cloth's own thickness while catching a sleeve's flare (spec default).
pub const DEFAULT_HANG_CM: f32 = 3.0;

/// Links per chain — the Python's `CLOTH_SEGMENTS`, kept verbatim.
const SEGMENTS: u32 = 5;

/// Fewer welded members than this is a sliver or a stray shell, not a panel.
const MIN_REGION_VERTS: usize = 6;

/// The fraction of a chain's members, taken from its far end, whose centroid aims the chain — the
/// Python's `quantile(proj, 0.6)`.
const FAR_FRACTION: f32 = 0.4;

/// The fraction of a seamless region's members, taken from the end nearest its anchor bone, that
/// stands for the seam it has none of ([`nearest_the_anchor`]).
const ROOT_FRACTION: f32 = 0.1;

/// The mesh's connectivity at WELD level: the model arrives one vertex per triangle CORNER (the
/// [`crate::fbx::parse_fbx`] convention), so adjacency read through vertex indices finds none —
/// every corner is its own index. Welding by position is what makes a component a component.
struct Welded {
    /// Per vertex: its weld id.
    corner: Vec<u32>,
    welds: usize,
    /// Weld-level triangle edges (duplicated per incident triangle; neither pass cares).
    edges: Vec<(u32, u32)>,
}

fn welded(model: &RawModel) -> Welded {
    let (corner, positions) = weld_by_position(&model.vertices);
    let mut edges = Vec::with_capacity(model.indices.len());
    for t in model.indices.as_chunks::<3>().0 {
        let w = t.map(|i| corner.get(i as usize).copied().unwrap_or(u32::MAX));
        if w.contains(&u32::MAX) {
            continue;
        }
        for (x, y) in [(w[0], w[1]), (w[1], w[2]), (w[2], w[0])] {
            if x != y {
                edges.push((x, y));
            }
        }
    }
    Welded {
        corner,
        welds: positions.len(),
        edges,
    }
}

/// Split a garment into its hanging regions against the body it is worn on.
///
/// `hang_cm` is the threshold off the body's surface (see [`DEFAULT_HANG_CM`]); `body` is the
/// body's [`Flesh`] in the SAME space as the garment's vertices (a garment bake places both in the
/// body's world). The returned regions carry their membership and their anchor bone; their chains
/// and binds are laid by [`build_cloth`].
///
/// The anchor is the bone carrying the most skin weight over the region's BOUNDARY — the hanging
/// vertices that touch the rigid yoke, i.e. where the panel is actually attached, NOT its mass
/// centre (weighting the whole panel picks the bone under the hem, the POC's bug). With no weights
/// to read, the nearest bone (rest world position) to the boundary centroid stands in.
pub fn split_garment(garment: &RawModel, body: &Flesh, hang_cm: f32) -> Vec<ClothRegion> {
    if garment.vertices.is_empty() || garment.bones.is_empty() {
        return Vec::new();
    }
    let w = welded(garment);
    // Hanging at WELD level: every corner at one position shares its position's verdict.
    let mut hangs = vec![false; w.welds];
    for (i, v) in garment.vertices.iter().enumerate() {
        if body.distance_outside(Vec3::from_array(v.p)) > hang_cm {
            if let Some(&c) = w.corner.get(i) {
                hangs[c as usize] = true;
            }
        }
    }
    // The hanging graph, plus the BOUNDARY: a hanging weld that shares a triangle edge with a
    // rigid one is where this panel hangs FROM.
    let mut adj: Vec<Vec<u32>> = vec![Vec::new(); w.welds];
    let mut boundary = vec![false; w.welds];
    for &(a, b) in &w.edges {
        match (hangs[a as usize], hangs[b as usize]) {
            (true, true) => adj[a as usize].push(b),
            (true, false) => boundary[a as usize] = true,
            (false, true) => boundary[b as usize] = true,
            (false, false) => {}
        }
    }

    // Connected components of the hanging set — the regions.
    let mut comp = vec![u32::MAX; w.welds];
    let mut components: Vec<Vec<u32>> = Vec::new();
    for seed in 0..w.welds {
        if !hangs[seed] || comp[seed] != u32::MAX {
            continue;
        }
        let id = components.len() as u32;
        let mut stack = vec![seed as u32];
        let mut members = Vec::new();
        comp[seed] = id;
        while let Some(x) = stack.pop() {
            members.push(x);
            for &y in &adj[x as usize] {
                if comp[y as usize] == u32::MAX {
                    comp[y as usize] = id;
                    stack.push(y);
                }
            }
        }
        components.push(members);
    }

    let globals = rest_world_frames(garment);
    let heads: Vec<Vec3> = globals.iter().map(|g| g.w_axis.truncate()).collect();
    let mut out = Vec::new();
    for members in &components {
        if members.len() < MIN_REGION_VERTS {
            continue;
        }
        // Membership is the VERTICES (what every consumer indexes), the welds expanded back out.
        let mut verts: Vec<u32> = Vec::new();
        let mut edge_verts: Vec<u32> = Vec::new();
        let id = comp[members[0] as usize];
        for (i, &c) in w.corner.iter().enumerate() {
            if comp[c as usize] == id {
                verts.push(i as u32);
                if boundary[c as usize] {
                    edge_verts.push(i as u32);
                }
            }
        }
        // A component that touches nothing rigid (a free shell) is attached along all of itself.
        let edge: &[u32] = if edge_verts.is_empty() {
            &verts
        } else {
            &edge_verts
        };
        let anchor_bone = anchor_of(garment, &heads, edge);
        out.push(ClothRegion {
            name: format!("cloth_{:02}", out.len() + 1),
            anchor_bone,
            tag: RegionTag::Cloth,
            verts,
            chain_count: 1,
            params: ClothParams::default(),
            chains: Vec::new(),
            binds: Vec::new(),
        });
    }
    out
}

/// The bone a set of boundary vertices hangs from: most skin weight over them, else the nearest
/// bone to their centroid.
fn anchor_of(model: &RawModel, heads: &[Vec3], edge: &[u32]) -> String {
    let mut weight: HashMap<u32, f32> = HashMap::new();
    let mut centroid = Vec3::ZERO;
    for &v in edge {
        let Some(vert) = model.vertices.get(v as usize) else {
            continue;
        };
        centroid += Vec3::from_array(vert.p);
        for (j, wgt) in vert.joints.iter().zip(&vert.weights) {
            if *wgt > 0.0 && (*j as usize) < model.bones.len() {
                *weight.entry(*j).or_default() += *wgt;
            }
        }
    }
    if let Some((&j, _)) = weight
        .iter()
        .max_by(|a, b| a.1.total_cmp(b.1).then(b.0.cmp(a.0)))
    {
        return model.bones[j as usize].name.clone();
    }
    let centroid = centroid / edge.len().max(1) as f32;
    heads
        .iter()
        .enumerate()
        .min_by(|a, b| {
            a.1.distance_squared(centroid)
                .total_cmp(&b.1.distance_squared(centroid))
        })
        .map(|(i, _)| model.bones[i].name.clone())
        .unwrap_or_default()
}

/// Lay `region.chain_count` chains across the region's attachment edge and bind every member
/// vertex to its nearest one — the Python's `_region_chains` + nearest-chain bind, with the comb
/// rooted on the EDGE rather than all on the bone.
///
/// `chain_count == 0` leaves the region with no chains and no binds: it skins rigidly to its
/// anchor bone. Replaces whatever the region carried (the binds are DERIVED — rebuilt at bake).
pub fn build_cloth(model: &RawModel, region: &mut ClothRegion) {
    region.chains.clear();
    region.binds.clear();
    let k = region.chain_count as usize;
    let members: Vec<u32> = region
        .verts
        .iter()
        .copied()
        .filter(|&v| (v as usize) < model.vertices.len())
        .collect();
    if k == 0 || members.len() < 3 {
        return;
    }
    let p = |v: u32| Vec3::from_array(model.vertices[v as usize].p);
    let mean = |vs: &[u32]| {
        vs.iter().map(|&v| p(v)).fold(Vec3::ZERO, |a, b| a + b) / vs.len().max(1) as f32
    };
    let edge = attachment_edge(model, &members);
    let edge = if edge.len() < 2 {
        nearest_the_anchor(model, &region.anchor_bone, &members)
    } else {
        edge
    };
    let edge: &[u32] = if edge.len() < 2 { &members } else { &edge };
    let base = mean(edge);
    // THE HANG: from the attachment edge toward the region's far end.
    let far = members
        .iter()
        .copied()
        .max_by(|&a, &b| {
            p(a).distance_squared(base)
                .total_cmp(&p(b).distance_squared(base))
        })
        .expect("members is non-empty");
    let hang = (p(far) - base).normalize_or_zero();
    if hang == Vec3::ZERO {
        return;
    }
    // THE COMB'S AXIS: the attachment edge's own principal direction with the hang taken out —
    // the width the chains spread along. Read as the edge point farthest across the hang, which
    // needs no eigensolver and is exact for the flat edge a panel is actually cut on.
    let across = |v: u32| {
        let d = p(v) - base;
        d - hang * d.dot(hang)
    };
    let axis = edge
        .iter()
        .copied()
        .max_by(|&a, &b| {
            across(a)
                .length_squared()
                .total_cmp(&across(b).length_squared())
        })
        .map(|v| across(v).normalize_or_zero())
        .unwrap_or(Vec3::ZERO);
    // One chain, or an edge with no width at all: the comb collapses to a single hang.
    let (axis, k) = if axis == Vec3::ZERO {
        (Vec3::ZERO, 1)
    } else {
        (axis, k)
    };

    let along = |v: u32| (p(v) - base).dot(axis);
    let (lo, hi) = members.iter().fold((f32::MAX, f32::MIN), |(l, h), &v| {
        let t = along(v);
        (l.min(t), h.max(t))
    });
    let width = (hi - lo).max(1e-6);
    let bin = |v: u32| (((along(v) - lo) / width * k as f32) as usize).min(k - 1);

    // Per bin: the chain rooted on that slice of the attachment edge, aimed at its own far end.
    let mut extents: Vec<f32> = Vec::new();
    for j in 0..k {
        let mut mine: Vec<u32> = members.iter().copied().filter(|&v| bin(v) == j).collect();
        if mine.len() < 3 {
            continue;
        }
        let mine_edge: Vec<u32> = edge.iter().copied().filter(|&v| bin(v) == j).collect();
        let anchor = if mine_edge.is_empty() {
            base + axis * (lo + width * (j as f32 + 0.5) / k as f32)
        } else {
            mean(&mine_edge)
        };
        mine.sort_by(|&a, &b| {
            (p(a) - anchor)
                .dot(hang)
                .total_cmp(&(p(b) - anchor).dot(hang))
        });
        let cut = mine.len() - (mine.len() as f32 * FAR_FRACTION).ceil().max(1.0) as usize;
        let dir = (mean(&mine[cut..]) - anchor).normalize_or_zero();
        if dir == Vec3::ZERO {
            continue;
        }
        let extent = mine
            .iter()
            .map(|&v| (p(v) - anchor).dot(dir))
            .fold(0.0_f32, f32::max);
        if extent < 1e-3 {
            continue;
        }
        region.chains.push(ClothChain {
            anchor: anchor.to_array(),
            dir: dir.to_array(),
            seg_len: extent / SEGMENTS as f32,
            segments: SEGMENTS,
        });
        extents.push(extent);
    }
    if region.chains.is_empty() {
        return;
    }
    // THE BIND: every member rides its NEAREST chain (perpendicular distance to the ray), at the
    // segment + fraction its projection lands on — verbatim from the Python.
    for &v in &members {
        let q = p(v);
        let mut best = (f32::MAX, 0usize, 0.0f32);
        for (c, chain) in region.chains.iter().enumerate() {
            let anchor = Vec3::from(chain.anchor);
            let dir = Vec3::from(chain.dir);
            let rel = q - anchor;
            let t = rel.dot(dir).clamp(0.0, extents[c]);
            let d = (rel - dir * t).length();
            if d < best.0 {
                best = (d, c, t / chain.seg_len.max(1e-6));
            }
        }
        let (_, c, s) = best;
        let kk = (s.floor().max(0.0) as u32).min(SEGMENTS - 1);
        region.binds.push(ClothBind {
            v,
            c: c as u32,
            k: kk,
            f: (s - kk as f32).clamp(0.0, 1.0),
        });
    }
}

/// Where a region with NO seam hangs from: the [`ROOT_FRACTION`] of its members nearest the bone
/// it is anchored to. Separate cards — a fall of hair, a fan of feathers, a tassel that is its
/// own shell — share no triangle edge with what they lie on, and rooted on their own middle half
/// of them would hang rigid. Empty when the anchor names no bone of the model.
fn nearest_the_anchor(model: &RawModel, anchor_bone: &str, members: &[u32]) -> Vec<u32> {
    let Some(bone) = model.bones.iter().position(|b| b.name == anchor_bone) else {
        return Vec::new();
    };
    let (heads, tails) = crate::bake::bone_segments(model);
    let mut by: Vec<(f32, u32)> = members
        .iter()
        .map(|&v| {
            let p = Vec3::from_array(model.vertices[v as usize].p);
            let on = crate::bake::closest_point_segment(p, heads[bone], tails[bone]);
            (p.distance(on), v)
        })
        .collect();
    by.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
    by.truncate(((members.len() as f32 * ROOT_FRACTION).ceil() as usize).max(2));
    by.into_iter().map(|(_, v)| v).collect()
}

/// The members that touch a NON-member across a triangle edge — the seam the region hangs from.
fn attachment_edge(model: &RawModel, members: &[u32]) -> Vec<u32> {
    let w = welded(model);
    let mut mine = vec![false; w.welds];
    for &v in members {
        if let Some(&c) = w.corner.get(v as usize) {
            mine[c as usize] = true;
        }
    }
    let mut seam = vec![false; w.welds];
    for &(a, b) in &w.edges {
        match (mine[a as usize], mine[b as usize]) {
            (true, false) => seam[a as usize] = true,
            (false, true) => seam[b as usize] = true,
            _ => {}
        }
    }
    members
        .iter()
        .copied()
        .filter(|&v| w.corner.get(v as usize).is_some_and(|&c| seam[c as usize]))
        .collect()
}

// ── SELECTION (T2, the bench's region tagger) ───────────────────────────────────────────────
//
// The three ways a human names a region at the bench, each returning the same thing the auto-split
// returns — a [`ClothRegion`] carrying its membership and its anchor. Nothing here binds: the comb
// is [`build_cloth`]'s, run at bake from whatever the rows say.

/// THE GARMENT SPLIT AS THE BENCH RUNS IT: `garment` is the piece in its OWN space, `placement` is
/// the matrix the mount fit puts it on the body with, and `body` is the fitting body's mesh. The
/// measurement is [`split_garment`]'s, unchanged — this only carries the piece onto the body first
/// (the bake does the same through `attach_world`) and lends it the body's bones, which is what the
/// anchor is chosen from. `hang_cm` is the knob the bake pins at [`DEFAULT_HANG_CM`].
///
/// The returned membership indexes `garment.vertices` — the placed copy is vertex-for-vertex the
/// original, so a row the human then tags names the piece's own vertices.
pub fn split_worn(
    garment: &RawModel,
    placement: Mat4,
    body: &RawModel,
    hang_cm: f32,
) -> Vec<ClothRegion> {
    let normals = Mat3::from_mat4(placement).inverse().transpose();
    let placed = RawModel {
        vertices: garment
            .vertices
            .iter()
            .map(|v| RawVertex {
                p: placement.transform_point3(Vec3::from_array(v.p)).to_array(),
                n: (normals * Vec3::from_array(v.n))
                    .normalize_or_zero()
                    .to_array(),
                ..*v
            })
            .collect(),
        indices: garment.indices.clone(),
        // A garment arrives boneless (its skin comes from the body at bake); the body's bones are
        // the ones a panel can hang from, so they are what the anchor is read against.
        bones: body.bones.clone(),
        regions: Vec::new(),
    };
    split_garment(&placed, &Flesh::build_body(body), hang_cm)
}

/// The vertices a set of CULL planes hides — the bench's "select culled". A plane is
/// `(normal, d)` as the rig view's `cull_plane` states it: a point `x` is cut when
/// `normal · x > d`. A vertex ANY active plane hides is selected, because that is what the three
/// orthographic panels' chrome has between them taken away.
pub fn verts_beyond(model: &RawModel, planes: &[(Vec3, f32)]) -> Vec<u32> {
    if planes.is_empty() {
        return Vec::new();
    }
    model
        .vertices
        .iter()
        .enumerate()
        .filter(|(_, v)| {
            let p = Vec3::from_array(v.p);
            planes.iter().any(|&(n, d)| n.dot(p) > d)
        })
        .map(|(i, _)| i as u32)
        .collect()
}

/// A region authored by HAND from a vertex selection (a thin-part grow, a cull-plane pick). The
/// anchor is chosen by the same rule the auto-split uses — most skin weight over the selection,
/// else the nearest bone to its centroid — so a region has ONE anchor rule however it was made.
/// `chain_count` opens at 1 and the params at their defaults, the tagger's knobs to move.
pub fn region_from(model: &RawModel, name: String, tag: RegionTag, verts: Vec<u32>) -> ClothRegion {
    let heads: Vec<Vec3> = rest_world_frames(model)
        .iter()
        .map(|g| g.w_axis.truncate())
        .collect();
    ClothRegion {
        name,
        anchor_bone: anchor_of(model, &heads, &verts),
        tag,
        verts,
        chain_count: 1,
        params: ClothParams::default(),
        chains: Vec::new(),
        binds: Vec::new(),
    }
}

// ── THE BODY'S OWN APPENDAGES (ruling 7881216F) ─────────────────────────────────────────────

/// What a FLAT appendage opens with — FLESH, not cloth. The body holds its own ear up: no
/// gravity (the modelled pose already carries the weight), so it keeps its modelled shape at
/// rest where cloth drapes; it lags a turning head and SETTLES without a swing — a 60° snap
/// settles within 0.4 s and overshoots under 3°, where cloth (0.015 / 0.9) swings 25° past
/// (the gate `an_appendage_holds_its_shape_and_settles_without_a_swing`). Aaron 2026-10-08: ears
/// that jiggle like a hanging rag "are not right".
pub const FLESH: ClothParams = ClothParams {
    gravity: [0.0, 0.0, 0.0],
    stiffness: 0.01,
    damping: 0.7,
    iterations: 8,
    max_dt: 1.0 / 30.0,
};
/// The widest comb an appendage opens with.
const APPENDAGE_CHAINS: u32 = 4;
/// A FALL — the hair past a tail's last bone, the feathers past a wing's — is many cards lying
/// along each other, its largest under this share of it ([`crate::bake::Appendage::whole`]). A
/// horn modelled in two shells is still a horn.
const FALL_WHOLE: f32 = 1.0 / 3.0;

/// THE HAND-OFF: every boneless appendage the skin bind found ([`crate::bake::Seating::appendages`])
/// ARRIVES as a tagged region the Regions panel can flip — rigid with the bone it grows from
/// (`chain_count` 0: a horn, a tusk) or, when it is FLAT — a sheet by the shape graph's own
/// measure of one ([`crate::shape::SHEET_RATIO`]: an ear), or a FALL of cards, each of which is
/// one ([`FALL_WHOLE`]: a tail's hair) — soft on a comb of chains as wide as it is. Aaron:
/// *"Build the hand-off, flat defaults soft."*
///
/// TWINS ACROSS THE PLANE are read as ONE ([`crate::bake::Appendage::twin`]): the pair's
/// flatness is the median over BOTH their positions and its fall reading the lower of the two,
/// so two horns, two ears or a wing's feathers either side never arrive one soft and one rigid
/// on readings either side of the line.
///
/// An appendage any of whose vertices a region of `model` already holds is left to that region:
/// what a human tagged is never proposed again.
pub fn appendage_regions(model: &RawModel, found: &[crate::bake::Appendage]) -> Vec<ClothRegion> {
    let mut taken = vec![false; model.vertices.len()];
    for v in model.regions.iter().flat_map(|r| &r.verts) {
        if let Some(t) = taken.get_mut(*v as usize) {
            *t = true;
        }
    }
    // The reading an appendage opens on: its own, or its pair's pooled.
    let reading = |i: usize| -> (f32, f32) {
        let a = &found[i];
        match a.twin.and_then(|j| found.get(j)) {
            Some(b) => {
                let mut all: Vec<f32> = a.flats.iter().chain(&b.flats).copied().collect();
                all.sort_by(f32::total_cmp);
                (
                    all.get(all.len() / 2).copied().unwrap_or(a.flat),
                    a.whole.min(b.whole),
                )
            }
            None => (a.flat, a.whole),
        }
    };
    // One chain for every sheet's-worth of breadth: a plate as broad as two is a comb of two.
    let opens = |(flat, whole): (f32, f32)| {
        let sheets = flat / crate::shape::SHEET_RATIO;
        if sheets >= 1.0 || whole < FALL_WHOLE {
            (sheets.round() as u32).clamp(1, APPENDAGE_CHAINS)
        } else {
            0
        }
    };
    let mut out: Vec<ClothRegion> = Vec::new();
    for (i, a) in found.iter().enumerate() {
        let Some(bone) = model.bones.get(a.bone) else {
            continue;
        };
        if a.verts
            .iter()
            .any(|&v| taken.get(v as usize) != Some(&false))
        {
            continue;
        }
        let (flat, whole) = reading(i);
        let chain_count = opens((flat, whole));
        tracing::debug!(
            "appendage on {}: {} vertices, flat {:.2}, whole {:.2}{} → {} chain(s)",
            bone.name,
            a.verts.len(),
            a.flat,
            a.whole,
            a.twin.map_or(String::new(), |_| format!(
                " (with its twin: {flat:.2}, {whole:.2})"
            )),
            chain_count
        );
        out.push(ClothRegion {
            name: format!("appendage_{:02}", model.regions.len() + out.len() + 1),
            anchor_bone: bone.name.clone(),
            tag: RegionTag::Appendage,
            verts: a.verts.clone(),
            chain_count,
            params: FLESH,
            chains: Vec::new(),
            binds: Vec::new(),
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fbx::{RawBone, RawVertex};
    use crate::flesh::fixtures;

    fn bone(name: &str, at: Vec3) -> RawBone {
        RawBone {
            name: name.to_string(),
            parent: -1,
            translation: at.to_array(),
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale: [1.0; 3],
            inverse_bind: glam::Mat4::from_translation(-at).to_cols_array(),
        }
    }

    /// A tube body standing on Z, and a garment over it: a collar RING that hugs the tube, plus
    /// three panels hanging well clear of it — one at each of three heights, each welded to the
    /// ring only through the rigid cloth that touches the body.
    fn tube_body() -> RawModel {
        fixtures::tube(
            &[
                Vec3::new(0.0, 0.0, 0.0),
                Vec3::new(0.0, 0.0, 60.0),
                Vec3::new(0.0, 0.0, 120.0),
            ],
            &[10.0, 10.0, 10.0],
        )
    }

    /// A garment panel: a flat quad grid in the XZ plane at `y`, welded along its top edge to a
    /// strip lying ON the body tube (radius 10) so the split has a rigid yoke to cut against.
    fn garment(bones: Vec<RawBone>) -> RawModel {
        let mut verts: Vec<RawVertex> = Vec::new();
        let mut indices: Vec<u32> = Vec::new();
        // Three panels at three heights; each is a 4×4 grid whose TOP row sits on the tube's
        // surface (rigid) and whose lower rows hang 25 cm out from the axis (hanging).
        for (pi, top_z) in [100.0_f32, 70.0, 40.0].into_iter().enumerate() {
            let x = if pi == 1 { -1.0 } else { 1.0 };
            let grid: Vec<Vec3> = (0..4)
                .flat_map(|r| {
                    (0..4).map(move |c| {
                        let radius = if r == 0 { 10.0 } else { 25.0 };
                        Vec3::new(x * radius, -6.0 + 4.0 * c as f32, top_z - 6.0 * r as f32)
                    })
                })
                .collect();
            for r in 0..3 {
                for c in 0..3 {
                    let i = |rr: usize, cc: usize| grid[rr * 4 + cc];
                    for tri in [
                        [i(r, c), i(r + 1, c), i(r + 1, c + 1)],
                        [i(r, c), i(r + 1, c + 1), i(r, c + 1)],
                    ] {
                        for q in tri {
                            indices.push(verts.len() as u32);
                            verts.push(RawVertex {
                                p: q.to_array(),
                                n: [x, 0.0, 0.0],
                                uv: [0.0, 0.0],
                                joints: [pi as u32, 0, 0, 0],
                                weights: [1.0, 0.0, 0.0, 0.0],
                            });
                        }
                    }
                }
            }
        }
        RawModel {
            vertices: verts,
            indices,
            bones,
            regions: Vec::new(),
        }
    }

    fn three_bones() -> Vec<RawBone> {
        vec![
            bone("chest", Vec3::new(0.0, 0.0, 100.0)),
            bone("waist", Vec3::new(0.0, 0.0, 70.0)),
            bone("hip", Vec3::new(0.0, 0.0, 40.0)),
        ]
    }

    #[test]
    fn a_garment_splits_into_one_region_per_hanging_panel() {
        let body = Flesh::build(&tube_body());
        let g = garment(three_bones());
        let regions = split_garment(&g, &body, DEFAULT_HANG_CM);
        assert_eq!(regions.len(), 3, "three panels, three regions");
        // Each panel's verts are weighted to its own bone, and that is the anchor found on the
        // BOUNDARY (the row that lies on the body).
        let mut anchors: Vec<&str> = regions.iter().map(|r| r.anchor_bone.as_str()).collect();
        anchors.sort_unstable();
        assert_eq!(anchors, ["chest", "hip", "waist"]);
        // Every hanging vert in EXACTLY one region; every rigid vert in none.
        let mut seen = vec![0u32; g.vertices.len()];
        for r in &regions {
            for &v in &r.verts {
                seen[v as usize] += 1;
            }
        }
        for (i, v) in g.vertices.iter().enumerate() {
            let hanging = body.distance_outside(Vec3::from_array(v.p)) > DEFAULT_HANG_CM;
            assert_eq!(
                seen[i],
                u32::from(hanging),
                "vertex {i} at {:?} (hanging {hanging}) is in {} regions",
                v.p,
                seen[i]
            );
        }
        // The rigid top row sits ON the tube: it must read as rigid, not hanging.
        assert!(
            g.vertices
                .iter()
                .any(|v| body.distance_outside(Vec3::from_array(v.p)) <= DEFAULT_HANG_CM),
            "the yoke row hugs the body"
        );
    }

    #[test]
    fn a_vertex_inside_the_body_is_rigid() {
        let body = Flesh::build(&tube_body());
        assert_eq!(body.distance_outside(Vec3::new(0.0, 0.0, 60.0)), 0.0);
        assert!(body.distance_outside(Vec3::new(25.0, 0.0, 60.0)) > DEFAULT_HANG_CM);
    }

    #[test]
    fn build_cloth_lays_a_comb_and_binds_every_member() {
        let body = Flesh::build(&tube_body());
        let g = garment(three_bones());
        let mut regions = split_garment(&g, &body, DEFAULT_HANG_CM);
        for r in &mut regions {
            build_cloth(&g, r);
            assert!(!r.chains.is_empty(), "{} got no chain", r.name);
            assert_eq!(
                r.binds.len(),
                r.verts.len(),
                "{}: a bind per member",
                r.name
            );
            for b in &r.binds {
                assert!(b.k < SEGMENTS, "{}: segment {} out of range", r.name, b.k);
                assert!(
                    (0.0..=1.0).contains(&b.f),
                    "{}: f {} out of range",
                    r.name,
                    b.f
                );
                assert!((b.c as usize) < r.chains.len());
            }
            // The chains hang DOWN and OUT from the attachment edge, not from the far hem.
            for c in &r.chains {
                assert!(c.seg_len > 0.0 && c.segments == SEGMENTS);
                assert!(
                    Vec3::from(c.dir).z < 0.0,
                    "{}: the panel hangs downward",
                    r.name
                );
            }
        }
        // A WIDE panel combed at 3 gets three chains.
        let mut wide = regions.remove(0);
        wide.chain_count = 3;
        build_cloth(&g, &mut wide);
        assert_eq!(wide.chains.len(), 3, "chain_count chains across the panel");
        assert_eq!(wide.binds.len(), wide.verts.len());
        // and every chain is used by somebody (the comb spans the width).
        for c in 0..3u32 {
            assert!(
                wide.binds.iter().any(|b| b.c == c),
                "chain {c} bound nothing"
            );
        }
    }

    #[test]
    fn chain_count_zero_is_rigid_to_the_anchor() {
        let body = Flesh::build(&tube_body());
        let g = garment(three_bones());
        let mut r = split_garment(&g, &body, DEFAULT_HANG_CM).remove(0);
        r.chain_count = 0;
        build_cloth(&g, &mut r);
        assert!(r.chains.is_empty() && r.binds.is_empty());
        assert!(!r.anchor_bone.is_empty(), "it still skins to its bone");
    }

    #[test]
    fn with_no_weights_the_anchor_is_the_nearest_bone_to_the_seam() {
        let body = Flesh::build(&tube_body());
        let mut g = garment(three_bones());
        for v in &mut g.vertices {
            v.weights = [0.0; 4];
        }
        let regions = split_garment(&g, &body, DEFAULT_HANG_CM);
        let mut anchors: Vec<&str> = regions.iter().map(|r| r.anchor_bone.as_str()).collect();
        anchors.sort_unstable();
        assert_eq!(anchors, ["chest", "hip", "waist"]);
    }

    /// THE BENCH'S SPLIT (T2): the garment arrives BONELESS in its own space and the body is a
    /// mesh, not a field — [`split_worn`] carries it onto the body through the mount fit's matrix
    /// and lends it the body's bones, and the measurement is then the bake's own. Offsetting the
    /// placement by the same amount the garment is authored off by lands the identical rows, and
    /// the hang is a knob: opened wide enough, nothing hangs.
    #[test]
    fn the_bench_split_places_the_garment_on_the_body_and_the_hang_is_a_knob() {
        // The BODY is the fitting body: a mesh WITH the bones a panel can hang from.
        let body = RawModel {
            bones: three_bones(),
            ..tube_body()
        };
        let mut g = garment(Vec::new());
        assert!(g.bones.is_empty(), "a garment arrives boneless");
        let placed = split_worn(&g, Mat4::IDENTITY, &body, DEFAULT_HANG_CM);
        assert_eq!(placed.len(), 3, "three hanging panels");
        for r in &placed {
            assert!(
                three_bones().iter().any(|b| b.name == r.anchor_bone),
                "the anchor is one of the BODY's bones, got {}",
                r.anchor_bone
            );
        }

        // The same piece authored 30 cm off the body, put back by the fit's matrix.
        let shift = Vec3::new(0.0, 0.0, -30.0);
        for v in &mut g.vertices {
            v.p = (Vec3::from_array(v.p) + shift).to_array();
        }
        let refit = split_worn(&g, Mat4::from_translation(-shift), &body, DEFAULT_HANG_CM);
        assert_eq!(
            refit.iter().map(|r| r.verts.len()).collect::<Vec<_>>(),
            placed.iter().map(|r| r.verts.len()).collect::<Vec<_>>(),
            "the fit's placement is what the hang is measured in"
        );

        // THE HANG IS A KNOB. Measured on a body with a wide base, so the panels lie INSIDE the
        // field and read their real 15 cm standoff (past the body's own grid a vertex hangs by
        // definition, which is the tube alone's answer and says nothing about the threshold).
        let based = RawModel {
            bones: three_bones(),
            ..fixtures::merge(vec![
                tube_body(),
                fixtures::box_mesh(-40.0, 40.0, -40.0, 40.0, -2.0, 0.0),
            ])
        };
        assert_eq!(
            split_worn(&g, Mat4::from_translation(-shift), &based, 3.0).len(),
            3
        );
        assert!(
            split_worn(&g, Mat4::from_translation(-shift), &based, 20.0).is_empty(),
            "a hang wider than the panels stand off leaves nothing hanging"
        );
    }

    /// SELECT CULLED (T2): the panels' cut planes state `normal · x > d` for what they hide, and
    /// the selection is exactly that — here one plane through the tube's axis, which takes the two
    /// panels on the far side and leaves the near one alone.
    #[test]
    fn select_culled_takes_exactly_what_the_cut_plane_hides() {
        let g = garment(three_bones());
        let plane = (Vec3::X, 0.0);
        let culled = verts_beyond(&g, &[plane]);
        assert!(
            !culled.is_empty() && culled.len() < g.vertices.len(),
            "a cut"
        );
        for (i, v) in g.vertices.iter().enumerate() {
            let beyond = v.p[0] > 0.0;
            assert_eq!(
                culled.contains(&(i as u32)),
                beyond,
                "vertex {i} at x = {} is {}",
                v.p[0],
                if beyond { "cut" } else { "kept" }
            );
        }
        assert!(verts_beyond(&g, &[]).is_empty(), "no cut selects nothing");

        // A hand-authored region off that selection carries the tag and an anchor off the ONE rule.
        let r = region_from(&g, "hair_01".into(), RegionTag::Hair, culled.clone());
        assert_eq!(r.tag, RegionTag::Hair);
        assert_eq!(r.verts, culled);
        assert_eq!(r.chain_count, 1);
        assert!(three_bones().iter().any(|b| b.name == r.anchor_bone));
    }

    #[test]
    fn an_unworn_garment_splits_into_nothing_when_it_hugs_the_body() {
        // The same garment against a body big enough to swallow it: nothing hangs.
        let fat = fixtures::tube(
            &[Vec3::new(0.0, 0.0, 0.0), Vec3::new(0.0, 0.0, 120.0)],
            &[40.0, 40.0],
        );
        let g = garment(three_bones());
        assert!(split_garment(&g, &Flesh::build(&fat), DEFAULT_HANG_CM).is_empty());
    }

    /// A card strip that touches nothing — a fall of hair, a feather — has no seam; its comb is
    /// rooted at the end nearest its anchor bone and hangs from there, not from its own middle.
    #[test]
    fn a_seamless_region_hangs_from_the_end_nearest_its_anchor() {
        let mut verts: Vec<RawVertex> = Vec::new();
        let mut indices: Vec<u32> = Vec::new();
        let at =
            |r: usize, c: usize| Vec3::new(if c == 0 { -2.0 } else { 2.0 }, 0.0, -5.0 * r as f32);
        for r in 0..10 {
            for tri in [
                [at(r, 0), at(r + 1, 0), at(r + 1, 1)],
                [at(r, 0), at(r + 1, 1), at(r, 1)],
            ] {
                for q in tri {
                    indices.push(verts.len() as u32);
                    verts.push(RawVertex {
                        p: q.to_array(),
                        n: [0.0, 1.0, 0.0],
                        uv: [0.0, 0.0],
                        joints: [0, 0, 0, 0],
                        weights: [1.0, 0.0, 0.0, 0.0],
                    });
                }
            }
        }
        let n = verts.len() as u32;
        let model = RawModel {
            vertices: verts,
            indices,
            bones: vec![bone("tail", Vec3::new(0.0, 0.0, 5.0))],
            regions: Vec::new(),
        };
        let mut region = region_from(
            &model,
            "fall".into(),
            RegionTag::Appendage,
            (0..n).collect(),
        );
        assert_eq!(region.anchor_bone, "tail");
        build_cloth(&model, &mut region);
        assert_eq!(region.chains.len(), 1, "one chain");
        let chain = &region.chains[0];
        assert!(
            chain.anchor[2] > -6.0,
            "rooted at the top, by the bone: {:?}",
            chain.anchor
        );
        assert!(
            chain.dir[2] < -0.95,
            "and hanging down the strip: {:?}",
            chain.dir
        );
        assert!(
            (chain.seg_len * chain.segments as f32 - 50.0).abs() < 6.0,
            "the whole strip's length: {}",
            chain.seg_len * chain.segments as f32
        );
        assert_eq!(region.binds.len(), n as usize, "every member bound");
    }

    /// The hand-off's defaults: a round one-piece appendage opens rigid, a flat one soft, a FALL of
    /// cards soft though its flesh reads round — and twins across the plane open alike, as the
    /// softer of the two reads.
    #[test]
    fn appendage_defaults_read_flatness_falls_and_twins() {
        use crate::bake::Appendage;
        let model = RawModel {
            vertices: (0..40)
                .map(|i| RawVertex {
                    p: [i as f32, 0.0, 0.0],
                    n: [0.0, 0.0, 1.0],
                    uv: [0.0, 0.0],
                    joints: [0, 0, 0, 0],
                    weights: [1.0, 0.0, 0.0, 0.0],
                })
                .collect(),
            indices: Vec::new(),
            bones: vec![
                bone("hand_l", Vec3::new(10.0, 0.0, 0.0)),
                bone("hand_r", Vec3::new(-10.0, 0.0, 0.0)),
                bone("head", Vec3::new(0.0, -10.0, 0.0)),
                bone("tail_hair_05", Vec3::new(0.0, 10.0, 0.0)),
            ],
            regions: Vec::new(),
        };
        let piece = |bone: usize,
                     verts: std::ops::Range<u32>,
                     flats: Vec<f32>,
                     whole: f32,
                     twin: Option<usize>| {
            let mut flats = flats;
            flats.sort_by(f32::total_cmp);
            Appendage {
                bone,
                verts: verts.collect(),
                flat: flats[flats.len() / 2],
                flats,
                whole,
                twin,
            }
        };
        let found = vec![
            // A wing's feathers reading just under a sheet on the left, well over on the right:
            // read as one pair they are a sheet.
            piece(0, 0..10, vec![2.9, 2.9, 2.9], 1.0, Some(1)),
            piece(1, 10..19, vec![3.3, 3.6, 3.9, 4.0], 1.0, Some(0)),
            // A horn: round, one shell.
            piece(2, 20..25, vec![1.5], 1.0, None),
            // A tail's fall: round as a bundle, but cut into many cards.
            piece(3, 25..40, vec![1.5], 0.2, None),
        ];
        let regions = appendage_regions(&model, &found);
        let got: Vec<(&str, u32)> = regions
            .iter()
            .map(|r| (r.anchor_bone.as_str(), r.chain_count))
            .collect();
        assert_eq!(
            got,
            vec![
                ("hand_l", 1),
                ("hand_r", 1),
                ("head", 0),
                ("tail_hair_05", 1)
            ],
            "{regions:#?}"
        );
        assert!(regions.iter().all(|r| r.tag == RegionTag::Appendage
            && r.params.stiffness == FLESH.stiffness
            && r.params.gravity == [0.0; 3]));
        // The left wing alone, with no twin to read against, is rigid by its own reading.
        let mut alone = found[..1].to_vec();
        alone[0].twin = None;
        assert_eq!(appendage_regions(&model, &alone)[0].chain_count, 0);
        // A pair is read over BOTH its bodies of positions: a small flat piece against a large
        // round twin reads round, and both stay rigid.
        let horns = vec![
            piece(2, 0..17, vec![1.6; 17], 1.0, Some(1)),
            piece(2, 17..26, vec![3.2; 9], 1.0, Some(0)),
        ];
        assert!(
            appendage_regions(&model, &horns)
                .iter()
                .all(|r| r.chain_count == 0),
            "both horns rigid"
        );
    }

    /// AN APPENDAGE IS FLESH, NOT CLOTH (Aaron 2026-10-08, on an aurochs's ears: they jiggle and
    /// "are not right"): on [`FLESH`] a chain keeps its modelled shape at rest — no droop, where
    /// cloth's hangs — and, snapped 60° with the head, settles within 0.4 s overshooting under 3°,
    /// where cloth's swings 10° and more past.
    #[test]
    fn an_appendage_holds_its_shape_and_settles_without_a_swing() {
        use flicker_skeletal::jiggle::{JiggleChain, JiggleParams};
        use glam::Quat;
        // A 20-cm ear of five links, out of the head's side; then the head snaps 60° about z.
        let run = |p: ClothParams| {
            let (anchor, dir, len) = (Vec3::new(0.0, 0.0, 100.0), Vec3::X, 20.0);
            let mut c = JiggleChain::new(anchor, dir, len / 5.0, 5, JiggleParams::from(p));
            for _ in 0..240 {
                c.step(anchor, Quat::IDENTITY, 1.0 / 60.0);
            }
            let droop = (c.positions()[5] - (anchor + dir * len)).length();
            let rot = Quat::from_rotation_z(60f32.to_radians());
            let target = anchor + rot * dir * len;
            let (mut settled, mut overshoot) = (None, 0.0f32);
            for k in 0..60 {
                c.step(anchor, rot, 1.0 / 60.0);
                let tip = c.positions()[5];
                let ang = (tip - anchor)
                    .normalize()
                    .dot((target - anchor).normalize())
                    .clamp(-1.0, 1.0)
                    .acos()
                    .to_degrees();
                // Past the target in the turn's own direction.
                if (tip - anchor).cross(target - anchor).z < 0.0 {
                    overshoot = overshoot.max(ang);
                }
                if settled.is_none() && (tip - target).length() < 0.02 * len {
                    settled = Some(k);
                }
            }
            (droop, settled, overshoot)
        };
        let (droop, settled, overshoot) = run(FLESH);
        assert!(
            droop < 1e-3,
            "flesh holds its modelled shape at rest: droop {droop:.3} cm"
        );
        assert!(
            settled.is_some_and(|k| k <= 24),
            "flesh settles within 0.4 s of a 60° snap: {settled:?} steps"
        );
        assert!(
            overshoot < 3.0,
            "flesh settles without a swing: overshoot {overshoot:.1}°"
        );
        let (droop, _, overshoot) = run(ClothParams::default());
        assert!(
            droop > 1.0 && overshoot > 10.0,
            "cloth drapes and swings: droop {droop:.1} cm, overshoot {overshoot:.1}°"
        );
    }
}
