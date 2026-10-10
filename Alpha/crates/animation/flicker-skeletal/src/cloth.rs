//! Runtime dynamic cloth — two solvers over the one region contract, picked by the region's TAG.
//!
//! **The COMB** (`Hair`, `Mane`, `Tail`, `Pendant`, `Appendage`): the region's vertices FOLLOW
//! jiggle chains — the strand solver, and the flesh one on the `Appendage` preset. **The SHEET**
//! (`Cloth`, ruling 82EDC071): cloth proper, position-based dynamics on the region's OWN
//! triangles — see [`Sheet`].
//!
//! The comb, as it has always been:
//! The rigid skin places a dangly region (a bell sleeve, a skirt hem) by its body bone,
//! which reads as stiff — and merely *adding* a swing on top reads as jello (the modelled
//! shape, quivering). Instead each cloth vertex is POSITIONED by a [`JiggleChain`] hung from
//! the region's anchor bone: it rides a point along the chain plus its own fixed offset from
//! that chain (the tube cross-section / flare), rotated to follow the chain's bend. The
//! chain hangs under gravity and lags the bone, so the whole region DRAPES downward out of
//! its modelled pose and swings — it collapses into a hanging shape instead of holding the
//! pose and wobbling.
//!
//! Reuses [`JiggleChain`] verbatim (the necklace is its other user); this module is the
//! per-vertex binding + per-frame placement over the rigidly-skinned buffer (only bound
//! verts are overwritten, so the rest of the garment keeps its rigid skin exactly).

use std::collections::{hash_map::Entry, HashMap, HashSet};

use glam::{Mat4, Quat, Vec3};

use crate::format::{
    Bone, Cloth, ClothParams, ClothRegion, CollisionShape, CollisionVolume, RegionTag, Vertex,
};
use crate::jiggle::{JiggleChain, JiggleParams};
use crate::skin::SkinnedVertex;

/// The file's physics dials, as the solver takes them — the ONE conversion.
impl From<ClothParams> for JiggleParams {
    fn from(p: ClothParams) -> Self {
        Self {
            gravity: Vec3::from(p.gravity),
            stiffness: p.stiffness,
            damping: p.damping,
            iterations: p.iterations,
            max_dt: p.max_dt,
        }
    }
}

/// Settle passes when relaxing a chain to its bind gravity-hang at build (one-time). Also what a
/// POSTER surface steps before its single upload, so a still image is already draped (6C46CAB9).
pub const SETTLE_STEPS: usize = 240;

/// One vertex's attachment to ONE chain: where along it, and the vertex's own offset from that
/// point in bind space (the tube cross-section / flare), rotated by the segment's bend each frame.
#[derive(Clone, Copy)]
struct Attach {
    chain: usize,
    k: usize,
    f: f32,
    offset: Vec3,
}

struct Bind {
    v: usize,
    /// The vertex's NEAREST chain — the one the file bound it to.
    a: Attach,
    /// Its SECOND-nearest chain of the same region (the COMB, EC30FD2E). Equal to `a` when the
    /// region hangs on a single chain.
    b: Attach,
    /// How much of `b` the vertex takes, from the inverse LATERAL distance to the two chains:
    /// `d1 / (d1 + d2)`. A vertex midway between two chains reads 0.5 and lands midway, so a wide
    /// panel reads as a SHEET instead of swinging as one stiff strip. 0 for a single-chain region.
    w2: f32,
    /// Bind-space vertex normal, rotated the same way so lighting follows the drape.
    normal: Vec3,
}

/// A body capsule the cloth's free nodes are pushed out of — the rig's own `collision` volumes
/// (`CollisionShape::Capsule`), resolved ONCE into BIND space so a frame costs one transform by
/// the bone's palette matrix. The overlap test itself is NOT here: `flicker-mechanics` depends on
/// this crate, so the geometry layer is unreachable from it without a cycle. The runtime hands
/// each node to [`ClothSim::push_free_nodes`] and does the one `collision::penetration` there.
#[derive(Clone, Copy, Debug)]
pub struct ClothCapsule {
    /// Index into the skeleton — the bone whose palette matrix poses the capsule.
    pub bone: usize,
    pub a: Vec3,
    pub b: Vec3,
    pub radius: f32,
}

struct Region {
    anchor_bone: usize,
    chains: Vec<JiggleChain>,
    /// Per chain: the chain's bind-space anchor point (`positions()[0]` at rest).
    anchor_bind: Vec<Vec3>,
    /// Per chain: the straight rest direction (bind space) — the "no-bend" reference the
    /// per-segment bend rotation is measured against each frame.
    rest_dir: Vec<Vec3>,
    binds: Vec<Bind>,
    /// Per chain, REUSED every frame: the posed points and the per-segment bend rotation. Sized
    /// once at build and overwritten in place — a game engine has no frames to spend on two
    /// nested `Vec` allocations per region per frame (405F7034).
    dyn_pts: Vec<Vec<Vec3>>,
    seg_rot: Vec<Vec<Quat>>,
    /// The anchor bone's rotation, cached by [`ClothSim::step`] for [`ClothSim::place`].
    driver_rot: Quat,
    /// False until the region has been stepped, so `place` never reads an empty frame.
    posed: bool,
}

/// The dynamic-cloth state for one garment: regions of jiggle chains + the vertices that
/// ride them. Built once from the `cloth` metadata + the bind-pose mesh; stepped and applied
/// every frame.
pub struct ClothSim {
    regions: Vec<Region>,
    sheets: Vec<Sheet>,
    capsules: Vec<ClothCapsule>,
}

impl ClothSim {
    /// Build from a garment's `cloth` metadata, its bind-pose `verts` (each bound vertex's
    /// rest position + normal → its offset from the chain), its triangle list `tris` over those
    /// same vertices (what a SHEET is built on; the comb reads none of it), and the base
    /// skeleton. Regions whose anchor bone is missing from `bones` are skipped. Chains and
    /// sheets are settled to their bind gravity-hang here so the first frame starts already
    /// draped (no startup pop).
    pub fn build(cloth: &Cloth, verts: &[Vertex], tris: &[u32], bones: &[Bone]) -> Self {
        let mut regions = Vec::new();
        let mut sheets = Vec::new();
        for r in &cloth.regions {
            let Some(anchor_bone) = bones.iter().position(|b| b.name == r.anchor_bone) else {
                eprintln!(
                    "flicker-skeletal: cloth region '{}' anchor bone '{}' not in skeleton; skipped",
                    r.name, r.anchor_bone
                );
                continue;
            };
            // THE TAG PICKS THE SOLVER: a `Cloth` region is a sheet over its own triangles; a
            // region of none (`chain_count` 0) is rigid to its anchor, as it always was.
            if r.tag == RegionTag::Cloth {
                if r.chain_count == 0 {
                    continue;
                }
                match Sheet::build(r, verts, tris, anchor_bone, bones) {
                    Some(sheet) => sheets.push(sheet),
                    None => eprintln!(
                        "flicker-skeletal: cloth region '{}' reaches no triangle of its mesh; skipped",
                        r.name
                    ),
                }
                continue;
            }
            let params = JiggleParams::from(r.params.clone());
            let mut chains = Vec::new();
            let mut anchor_bind = Vec::new();
            let mut rest_dir = Vec::new();
            for c in &r.chains {
                let a = Vec3::from(c.anchor);
                let dir = Vec3::from(c.dir).normalize_or_zero();
                let mut chain = JiggleChain::new(a, dir, c.seg_len, c.segments as usize, params);
                for _ in 0..SETTLE_STEPS {
                    chain.step(a, Quat::IDENTITY, 1.0 / 60.0);
                }
                chains.push(chain);
                anchor_bind.push(a);
                rest_dir.push(dir);
            }
            // The COMB: every vertex's attachment to its own chain AND to the second-nearest
            // chain of the region, both projected here at BUILD so a frame is two lerps.
            let axes: Vec<(Vec3, Vec3, f32, f32)> = r
                .chains
                .iter()
                .map(|c| {
                    let a = Vec3::from(c.anchor);
                    let dir = Vec3::from(c.dir).normalize_or_zero();
                    (a, dir, c.seg_len, c.seg_len * c.segments as f32)
                })
                .collect();
            let binds = r
                .binds
                .iter()
                .filter_map(|b| {
                    let ci = b.c as usize;
                    let (a, dir, seg_len, _) = *axes.get(ci)?;
                    let vert = verts.get(b.v as usize)?;
                    let p = Vec3::from(vert.p);
                    let base_rest = a + dir * (seg_len * (b.k as f32 + b.f));
                    let first = Attach {
                        chain: ci,
                        k: b.k as usize,
                        f: b.f,
                        offset: p - base_rest,
                    };
                    // Second-nearest chain by LATERAL distance (perpendicular to the hang), which
                    // is what "across the panel" means; distance ALONG the chain is the vertex's
                    // own place on it and must not enter the choice.
                    let lateral = |ci: usize| {
                        let (a, dir, _, _) = axes[ci];
                        let d = p - a;
                        (d - dir * d.dot(dir)).length()
                    };
                    let d1 = lateral(ci);
                    let second = (0..axes.len())
                        .filter(|&j| j != ci)
                        .map(|j| (j, lateral(j)))
                        .min_by(|x, y| x.1.total_cmp(&y.1));
                    let (b2, w2) = match second {
                        Some((j, d2)) if d1 + d2 > 1e-6 => (project(p, &axes, j), d1 / (d1 + d2)),
                        _ => (first, 0.0),
                    };
                    Some(Bind {
                        v: b.v as usize,
                        a: first,
                        b: b2,
                        w2,
                        normal: Vec3::from(vert.n),
                    })
                })
                .collect();
            let dyn_pts = chains.iter().map(|c| Vec::with_capacity(c.len())).collect();
            let seg_rot = chains
                .iter()
                .map(|c| Vec::with_capacity(c.len().saturating_sub(1)))
                .collect();
            regions.push(Region {
                anchor_bone,
                chains,
                anchor_bind,
                rest_dir,
                binds,
                dyn_pts,
                seg_rot,
                driver_rot: Quat::IDENTITY,
                posed: false,
            });
        }
        Self {
            regions,
            sheets,
            capsules: Vec::new(),
        }
    }

    /// Take the rig's own `collision` capsules as the body the cloth cannot pass through (the
    /// front falls through the thighs the moment the character walks, EC30FD2E). Volumes are
    /// resolved by BONE NAME like every clip track, and baked into BIND space once — so a frame
    /// costs one `transformed(palette[bone])` per capsule and nothing else. A rig with no
    /// `collision` block (every rig from before the capsule bake) simply collides with nothing.
    pub fn set_capsules(&mut self, volumes: &[CollisionVolume], bones: &[Bone]) {
        self.capsules.clear();
        for v in volumes {
            let Some(bone) = bones.iter().position(|b| b.name == v.bone) else {
                continue;
            };
            // The volume is authored in the bone's LOCAL frame; the chains live in bind space.
            let to_bind = bones[bone].inverse_bind.inverse();
            let scale =
                (to_bind.x_axis.length() + to_bind.y_axis.length() + to_bind.z_axis.length()) / 3.0;
            let (a, b, radius) = match v.shape {
                CollisionShape::Capsule { a, b, radius } => (Vec3::from(a), Vec3::from(b), radius),
                CollisionShape::Sphere { center, radius } => {
                    (Vec3::from(center), Vec3::from(center), radius)
                }
                CollisionShape::Box { .. } => continue,
            };
            self.capsules.push(ClothCapsule {
                bone,
                a: to_bind.transform_point3(a),
                b: to_bind.transform_point3(b),
                radius: radius * scale,
            });
        }
    }

    /// The body capsules, bind space — the runtime poses them by `palette[bone]` and tests them
    /// with `flicker_mechanics::collision`.
    pub fn capsules(&self) -> &[ClothCapsule] {
        &self.capsules
    }

    /// True when there is nothing to simulate.
    pub fn is_empty(&self) -> bool {
        self.regions.iter().all(|r| r.binds.is_empty()) && self.sheets.is_empty()
    }

    /// The sheets as built and as they stand: per sheet its particles, pins and constraints, and
    /// the worst strain over its OWN stretch constraints right now — a diagnostic's reading, not
    /// a frame's.
    pub fn sheet_report(&self) -> String {
        self.sheets
            .iter()
            .map(|sh| {
                let mut degree = vec![0u32; sh.pos.len()];
                for c in &sh.stretch {
                    degree[c.a as usize] += 1;
                    degree[c.b as usize] += 1;
                }
                let strain = |c: &Pair| {
                    let d = sh.pos[c.a as usize].distance(sh.pos[c.b as usize]);
                    if c.rest > 1e-4 {
                        (d - c.rest).abs() / c.rest
                    } else {
                        0.0
                    }
                };
                let mut ranked: Vec<&Pair> = sh.stretch.iter().collect();
                ranked.sort_by(|x, y| strain(y).total_cmp(&strain(x)));
                let worst: Vec<String> = ranked
                    .iter()
                    .take(3)
                    .map(|c| {
                        format!(
                            "[{}-{} rest {:.3} now {:.3} degrees {}/{} pinned {}/{}]",
                            c.a,
                            c.b,
                            c.rest,
                            sh.pos[c.a as usize].distance(sh.pos[c.b as usize]),
                            degree[c.a as usize],
                            degree[c.b as usize],
                            sh.inv_mass[c.a as usize] == 0.0,
                            sh.inv_mass[c.b as usize] == 0.0
                        )
                    })
                    .collect();
                format!(
                    "sheet: {} particles ({} pinned), {} stretch + {} bend constraints, worst \
                     stretch strain now {:.1} % {}",
                    sh.pos.len(),
                    sh.pins.len(),
                    sh.stretch.len(),
                    sh.bend.len(),
                    ranked.first().map_or(0.0, |c| strain(c)) * 100.0,
                    worst.join(" ")
                )
            })
            .collect::<Vec<_>>()
            .join("; ")
    }

    /// Advance every chain from the current pose. Split from [`ClothSim::place`] so the runtime
    /// can push the nodes out of the body BETWEEN the two (a push after placement would be a lie),
    /// and so a POSTER can settle with N steps and ONE placement.
    pub fn step(&mut self, palette: &[Mat4], dt: f32) {
        for r in &mut self.regions {
            let Some(pa) = palette.get(r.anchor_bone).copied() else {
                continue;
            };
            r.driver_rot = Quat::from_mat4(&pa).normalize();
            for (ci, chain) in r.chains.iter_mut().enumerate() {
                chain.step(pa.transform_point3(r.anchor_bind[ci]), r.driver_rot, dt);
            }
            r.posed = true;
        }
        for sheet in &mut self.sheets {
            sheet.step(palette, dt);
        }
    }

    /// Hand every FREE chain node (the anchor is the bone's and never moves) to `push`, which
    /// returns where it should be instead — the collision projection. Kept as a callback because
    /// the capsule test lives in `flicker-mechanics`, which depends on this crate.
    pub fn push_free_nodes(&mut self, mut push: impl FnMut(Vec3) -> Vec3) {
        for r in &mut self.regions {
            for chain in &mut r.chains {
                for p in chain.positions_mut().iter_mut().skip(1) {
                    *p = push(*p);
                }
            }
        }
        for sheet in &mut self.sheets {
            sheet.push_free(&mut push);
        }
    }

    /// PLACE each bound vertex on its chains — the comb blend of its two attachments. `skinned`
    /// is overwritten in place for bound verts only, so non-cloth verts keep their rigid skin.
    pub fn place(&mut self, skinned: &mut [SkinnedVertex]) {
        for sheet in &self.sheets {
            sheet.place(skinned);
        }
        for r in &mut self.regions {
            if !r.posed {
                continue;
            }
            // Read the (possibly pushed) chains back into the reused per-frame buffers.
            for (ci, chain) in r.chains.iter().enumerate() {
                let d = &mut r.dyn_pts[ci];
                d.clear();
                d.extend_from_slice(chain.positions());
                let rest_dir_posed = (r.driver_rot * r.rest_dir[ci]).normalize_or_zero();
                let qs = &mut r.seg_rot[ci];
                qs.clear();
                for k in 0..d.len().saturating_sub(1) {
                    let dd = (d[k + 1] - d[k]).normalize_or_zero();
                    qs.push(if dd.length_squared() > 0.5 {
                        Quat::from_rotation_arc(rest_dir_posed, dd)
                    } else {
                        Quat::IDENTITY
                    });
                }
            }
            for b in &r.binds {
                let Some(pa) = point_on(r, &b.a) else {
                    continue;
                };
                let (p, frame) = if b.w2 > 0.0 {
                    match point_on(r, &b.b) {
                        Some(pb) => (pa.0.lerp(pb.0, b.w2), pa.1),
                        None => pa,
                    }
                } else {
                    pa
                };
                if let Some(sv) = skinned.get_mut(b.v) {
                    sv.position = p.to_array();
                    sv.normal = (frame * b.normal).normalize_or_zero().to_array();
                }
            }
        }
    }

    /// Step every chain from the current pose and PLACE each bound vertex on it — the
    /// uncollided one-shot path. `palette[b] = global[b] * inverse_bind[b]`.
    pub fn update(&mut self, palette: &[Mat4], dt: f32, skinned: &mut [SkinnedVertex]) {
        self.step(palette, dt);
        self.place(skinned);
    }
}

/// Where one attachment puts its vertex this frame, and the frame that rotates its offset:
/// `(position, frame)`. `None` when the chain is shorter than the attachment's segment.
fn point_on(r: &Region, at: &Attach) -> Option<(Vec3, Quat)> {
    let d = r.dyn_pts.get(at.chain)?;
    if at.k + 1 >= d.len() {
        return None;
    }
    let frame = r.seg_rot[at.chain][at.k] * r.driver_rot;
    Some((d[at.k].lerp(d[at.k + 1], at.f) + frame * at.offset, frame))
}

/// Project a bind-space point onto chain `j` — its segment, fraction and cross-section offset,
/// exactly the way the emitter binds a vertex to its own chain, so the two attachments of a comb
/// blend are the same kind of thing.
fn project(p: Vec3, axes: &[(Vec3, Vec3, f32, f32)], j: usize) -> Attach {
    let (a, dir, seg_len, extent) = axes[j];
    let t = (p - a).dot(dir).clamp(0.0, extent);
    let seg_len = seg_len.max(1e-6);
    let segments = (extent / seg_len).round().max(1.0) as usize;
    let k = ((t / seg_len).floor() as usize).min(segments - 1);
    let f = (t / seg_len - k as f32).clamp(0.0, 1.0);
    Attach {
        chain: j,
        k,
        f,
        offset: p - (a + dir * (seg_len * (k as f32 + f))),
    }
}

// ── THE SHEET ─────────────────────────────────────────────────────────────────────────────────

/// A stretch or bending constraint: keep particles `a` and `b` `rest` apart. `wa`/`wb` are the
/// shares of a correction each takes (its inverse mass over the pair's), fixed at build — a
/// pinned particle takes none.
#[derive(Clone, Copy, Debug)]
struct Pair {
    a: u32,
    b: u32,
    rest: f32,
    wa: f32,
    wb: f32,
}

/// The share of an island's members pinned to its anchor bone when it has no seam.
const ISLAND_PINS: f32 = 0.05;

/// A free particle's TETHER to the seam: it may never be farther from pinned particle `pin`
/// than `reach`, the length of the shortest path of edges between them at bind. Position-based
/// relaxation carries a correction about one edge a pass, so a panel forty edges deep would
/// otherwise hang stretched like a spring under its own weight; the tether (long-range
/// attachment, Kim et al. 2012) holds every particle within its own unstretched reach of where
/// it is sewn on, at one projection a particle.
#[derive(Clone, Copy, Debug)]
struct Tether {
    particle: u32,
    pin: u32,
    reach: f32,
}

/// Positions closer than this, in cm, are ONE particle: the meshes the bake writes are triangle
/// soups (every corner its own vertex), and a sheet is nothing without shared edges.
const WELD_CM: f32 = 1e-3;

/// CLOTH PROPER (ruling 82EDC071): a `Cloth`-tagged region as a position-based sheet over its
/// OWN triangles. The submesh's vertices are WELDED by position into particles (the bake writes
/// a soup: three vertices a triangle, none shared), and a particle is the region's when any
/// vertex welded into it is. Every triangle edge between two of them is a STRETCH constraint
/// held at its bind length; every interior edge (two member triangles either side) is a
/// BENDING constraint — the two particles across it held at their bind distance by the region's
/// `stiffness`, the fold's resistance the comb of chains never had (shear needs no constraint
/// of its own: a triangle's three edges carry it). Its SEAM — the particles that share a
/// triangle, or a position, with the body beside the region — is PINNED to the skin every frame,
/// so the panel hangs from the body and follows it; a region with no seam (an island the tagger
/// drew) pins the particles nearest its anchor bone's head instead. The free particles fall
/// under the region's gravity (verlet, `damping`), the constraints are relaxed `iterations`
/// times (Gauss-Seidel; a pinned particle is immovable, a correction becomes velocity next
/// step), and the runtime pushes them out of the body's capsules between `step` and `place`
/// exactly as it does the comb's nodes. Built with every buffer at its size; a frame allocates
/// nothing.
struct Sheet {
    /// Every member vertex of the submesh and the particle it is welded into — what `place`
    /// writes.
    placed: Vec<(u32, u32)>,
    /// Inverse mass per particle: 0 for a pinned one.
    inv_mass: Vec<f32>,
    /// The pinned particles, their bind vertices for the per-frame skin, and that skin (reused).
    pins: Vec<u32>,
    pin_bind: Vec<Vertex>,
    pin_skinned: Vec<SkinnedVertex>,
    pos: Vec<Vec3>,
    prev: Vec<Vec3>,
    stretch: Vec<Pair>,
    bend: Vec<Pair>,
    tethers: Vec<Tether>,
    params: JiggleParams,
    posed: bool,
}

impl Sheet {
    /// `None` when the region reaches no vertex of the submesh.
    fn build(
        r: &ClothRegion,
        verts: &[Vertex],
        tris: &[u32],
        anchor_bone: usize,
        bones: &[Bone],
    ) -> Option<Self> {
        let n = verts.len();
        // Membership: the region's own vertices, and whatever its binds name (a rig from before
        // membership was written carries only those).
        let mut member = vec![false; n];
        for v in r.verts.iter().chain(r.binds.iter().map(|b| &b.v)) {
            if let Some(m) = member.get_mut(*v as usize) {
                *m = true;
            }
        }
        if !member.iter().any(|&m| m) {
            return None;
        }
        // WELD every vertex of the submesh into a particle by position.
        let mut welded: HashMap<[i32; 3], u32> = HashMap::new();
        let mut weld = vec![0u32; n];
        let mut first: Vec<u32> = Vec::new(); // a vertex of each particle
        for (v, vert) in verts.iter().enumerate() {
            let key = [
                (vert.p[0] / WELD_CM).round() as i32,
                (vert.p[1] / WELD_CM).round() as i32,
                (vert.p[2] / WELD_CM).round() as i32,
            ];
            let id = *welded.entry(key).or_insert_with(|| {
                first.push(v as u32);
                first.len() as u32 - 1
            });
            weld[v] = id;
        }
        let count = first.len();
        // A particle is the region's when any vertex welded into it is (that vertex is the
        // particle's own: its weights are the panel's); it is on the SEAM when a body vertex is
        // welded into it as well.
        let mut member_vertex = vec![u32::MAX; count];
        let mut mixed = vec![false; count];
        for v in 0..n {
            if member[v] && member_vertex[weld[v] as usize] == u32::MAX {
                member_vertex[weld[v] as usize] = v as u32;
            }
        }
        for v in 0..n {
            if !member[v] && member_vertex[weld[v] as usize] != u32::MAX {
                mixed[weld[v] as usize] = true;
            }
        }
        // The simulated particles, compacted.
        let mut sim = vec![u32::MAX; count];
        let mut particle_vertex: Vec<u32> = Vec::new();
        for p in 0..count {
            if member_vertex[p] != u32::MAX {
                sim[p] = particle_vertex.len() as u32;
                particle_vertex.push(member_vertex[p]);
            }
        }
        let at = |q: u32| Vec3::from(verts[particle_vertex[q as usize] as usize].p);
        let key = |a: u32, b: u32| if a < b { (a, b) } else { (b, a) };
        let mut seam: Vec<bool> = (0..count)
            .filter(|&p| member_vertex[p] != u32::MAX)
            .map(|p| mixed[p])
            .collect();
        let mut edges: HashSet<(u32, u32)> = HashSet::new();
        // Each edge of a member triangle → the particle across it, until its twin arrives.
        let mut across: HashMap<(u32, u32), u32> = HashMap::new();
        let mut bend: Vec<Pair> = Vec::new();
        for t in tris.as_chunks::<3>().0 {
            let q: Vec<u32> = t
                .iter()
                .map(|&i| weld.get(i as usize).map_or(u32::MAX, |&w| sim[w as usize]))
                .collect();
            let whole = q.iter().all(|&x| x != u32::MAX);
            for k in 0..3 {
                let (a, b, c) = (q[k], q[(k + 1) % 3], q[(k + 2) % 3]);
                if a == u32::MAX {
                    continue;
                }
                if b == u32::MAX || c == u32::MAX {
                    // A member in a triangle with the body: where the panel is sewn on.
                    seam[a as usize] = true;
                }
                if b != u32::MAX && a != b {
                    edges.insert(key(a, b));
                }
                if whole && a != b {
                    match across.entry(key(a, b)) {
                        Entry::Occupied(o) => {
                            let d = *o.get();
                            if d != c {
                                bend.push(Pair {
                                    a: c,
                                    b: d,
                                    rest: at(c).distance(at(d)),
                                    wa: 0.5,
                                    wb: 0.5,
                                });
                            }
                        }
                        Entry::Vacant(v) => {
                            v.insert(c);
                        }
                    }
                }
            }
        }
        let mut stretch: Vec<Pair> = edges
            .into_iter()
            .map(|(a, b)| Pair {
                a,
                b,
                rest: at(a).distance(at(b)),
                wa: 0.5,
                wb: 0.5,
            })
            .collect();
        // Deterministic order: the hash set's is not, and PBD's answer depends on the order.
        stretch.sort_by_key(|c| (c.a, c.b));
        bend.sort_by_key(|c| (c.a, c.b));
        let particles = particle_vertex.len();
        if !seam.iter().any(|&s| s) {
            // An island: pin the particles nearest the anchor bone's head in bind space.
            let head = bones[anchor_bone].inverse_bind.inverse().w_axis.truncate();
            let mut order: Vec<u32> = (0..particles as u32).collect();
            order.sort_by(|&a, &b| at(a).distance(head).total_cmp(&at(b).distance(head)));
            let pins = ((particles as f32 * ISLAND_PINS).ceil() as usize)
                .max(3)
                .min(particles);
            for &q in &order[..pins] {
                seam[q as usize] = true;
            }
        }
        let pins: Vec<u32> = (0..particles as u32)
            .filter(|&q| seam[q as usize])
            .collect();
        let pin_bind: Vec<Vertex> = pins
            .iter()
            .map(|&q| verts[particle_vertex[q as usize] as usize].clone())
            .collect();
        let inv_mass: Vec<f32> = seam.iter().map(|&s| if s { 0.0 } else { 1.0 }).collect();
        // The shares a correction splits by, once: a pinned particle takes none of it, and a pair
        // of pinned particles is dropped — nothing to relax.
        let weigh = |c: &mut Pair| {
            let (wa, wb) = (inv_mass[c.a as usize], inv_mass[c.b as usize]);
            let w = wa + wb;
            if w > 0.0 {
                c.wa = wa / w;
                c.wb = wb / w;
            }
            w > 0.0
        };
        stretch.retain_mut(weigh);
        bend.retain_mut(weigh);
        let tethers = tethers_to_the_seam(particles, &seam, &stretch);
        let placed: Vec<(u32, u32)> = (0..n as u32)
            .filter(|&v| member[v as usize])
            .map(|v| (v, sim[weld[v as usize] as usize]))
            .collect();
        let pos: Vec<Vec3> = (0..particles as u32).map(at).collect();
        let mut sheet = Sheet {
            pin_skinned: Vec::with_capacity(pins.len()),
            prev: pos.clone(),
            pos,
            placed,
            inv_mass,
            pins,
            pin_bind,
            stretch,
            bend,
            tethers,
            params: JiggleParams::from(r.params.clone()),
            posed: false,
        };
        // Settle at the bind pose (the identity palette IS the bind pose) so the first frame
        // opens already draped, as a chain does.
        let bind_palette = vec![Mat4::IDENTITY; bones.len()];
        for _ in 0..SETTLE_STEPS {
            sheet.step(&bind_palette, 1.0 / 60.0);
        }
        sheet.posed = false;
        Some(sheet)
    }

    fn step(&mut self, palette: &[Mat4], dt: f32) {
        let dt = dt.clamp(0.0, self.params.max_dt);
        let g = self.params.gravity * dt * dt;
        let damp = self.params.damping;
        // The seam rides the skin.
        crate::skin::skin_subset(&self.pin_bind, palette, &mut self.pin_skinned);
        for (k, &q) in self.pins.iter().enumerate() {
            let here = Vec3::from(self.pin_skinned[k].position);
            self.pos[q as usize] = here;
            self.prev[q as usize] = here;
        }
        // Verlet over the free particles.
        for i in 0..self.pos.len() {
            if self.inv_mass[i] <= 0.0 {
                continue;
            }
            let vel = (self.pos[i] - self.prev[i]) * damp;
            self.prev[i] = self.pos[i];
            self.pos[i] += vel + g;
        }
        let k_bend = self.params.stiffness.clamp(0.0, 1.0);
        for _ in 0..self.params.iterations {
            for c in &self.stretch {
                relax(&mut self.pos, c, 1.0);
            }
            for c in &self.bend {
                relax(&mut self.pos, c, k_bend);
            }
            for t in &self.tethers {
                let anchor = self.pos[t.pin as usize];
                let d = self.pos[t.particle as usize] - anchor;
                let len = d.length();
                if len > t.reach {
                    self.pos[t.particle as usize] = anchor + d * (t.reach / len);
                }
            }
        }
        self.posed = true;
    }

    fn push_free(&mut self, push: &mut impl FnMut(Vec3) -> Vec3) {
        for (i, p) in self.pos.iter_mut().enumerate() {
            if self.inv_mass[i] > 0.0 {
                *p = push(*p);
            }
        }
    }

    /// Positions only: the runtime reads the sheet's normals off its own draped triangles.
    fn place(&self, skinned: &mut [SkinnedVertex]) {
        if !self.posed {
            return;
        }
        for &(v, q) in &self.placed {
            if let Some(sv) = skinned.get_mut(v as usize) {
                sv.position = self.pos[q as usize].to_array();
            }
        }
    }
}

/// Every free particle's tether to its NEAREST pinned particle along the stretch edges, with the
/// path's bind length as its reach — a multi-source shortest-path walk from the seam. A particle
/// no path reaches (a piece the tagger drew loose) gets none.
fn tethers_to_the_seam(particles: usize, seam: &[bool], stretch: &[Pair]) -> Vec<Tether> {
    use std::cmp::Ordering;
    use std::collections::BinaryHeap;
    let mut adjacency: Vec<Vec<(u32, f32)>> = vec![Vec::new(); particles];
    for c in stretch {
        adjacency[c.a as usize].push((c.b, c.rest));
        adjacency[c.b as usize].push((c.a, c.rest));
    }
    #[derive(PartialEq)]
    struct Next(f32, u32);
    impl Eq for Next {}
    impl PartialOrd for Next {
        fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
            Some(self.cmp(other))
        }
    }
    impl Ord for Next {
        fn cmp(&self, other: &Self) -> Ordering {
            other.0.total_cmp(&self.0)
        }
    }
    let mut reach = vec![f32::INFINITY; particles];
    let mut pin = vec![u32::MAX; particles];
    let mut heap = BinaryHeap::new();
    for (q, &pinned) in seam.iter().enumerate() {
        if pinned {
            reach[q] = 0.0;
            pin[q] = q as u32;
            heap.push(Next(0.0, q as u32));
        }
    }
    while let Some(Next(d, q)) = heap.pop() {
        if d > reach[q as usize] {
            continue;
        }
        for &(next, w) in &adjacency[q as usize] {
            let nd = d + w;
            if nd < reach[next as usize] {
                reach[next as usize] = nd;
                pin[next as usize] = pin[q as usize];
                heap.push(Next(nd, next));
            }
        }
    }
    (0..particles)
        .filter(|&q| !seam[q] && pin[q] != u32::MAX)
        .map(|q| Tether {
            particle: q as u32,
            pin: pin[q],
            reach: reach[q],
        })
        .collect()
}

/// One Gauss-Seidel projection of a distance constraint, split by the pair's fixed shares and
/// scaled by `k` (1 for stretch, the bending stiffness for a fold).
#[inline]
fn relax(pos: &mut [Vec3], c: &Pair, k: f32) {
    let (a, b) = (c.a as usize, c.b as usize);
    let delta = pos[b] - pos[a];
    let d2 = delta.length_squared();
    if d2 <= 1e-12 {
        return;
    }
    let d = d2.sqrt();
    let corr = delta * ((d - c.rest) / d * k);
    pos[a] += corr * c.wa;
    pos[b] -= corr * c.wb;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::{ClothBind, ClothChain, ClothParams, ClothRegion};

    fn bone(name: &str) -> Bone {
        Bone {
            name: name.into(),
            parent: -1,
            local: Mat4::IDENTITY,
            inverse_bind: Mat4::IDENTITY,
        }
    }
    fn sv(p: [f32; 3]) -> SkinnedVertex {
        SkinnedVertex {
            position: p,
            normal: [0.0, 1.0, 0.0],
        }
    }
    fn vtx(p: [f32; 3]) -> Vertex {
        Vertex {
            p,
            n: [0.0, 1.0, 0.0],
            uv: [0.0, 0.0],
            joints: [0; 4],
            weights: [1.0, 0.0, 0.0, 0.0],
        }
    }

    // A HORIZONTAL chain (+x) with 3 verts sitting ON it, so gravity drapes them off the
    // straight rest. Limp so gravity clearly wins.
    fn one_region_sim() -> (ClothSim, Vec<[f32; 3]>) {
        let orig = vec![[0.0, 0.0, 0.0], [10.0, 0.0, 0.0], [20.0, 0.0, 0.0]];
        let cloth = Cloth {
            regions: vec![ClothRegion {
                name: "r".into(),
                anchor_bone: "a".into(),
                tag: RegionTag::Hair,
                verts: Vec::new(),
                chain_count: 1,
                params: ClothParams {
                    gravity: [0.0, 0.0, -600.0],
                    stiffness: 0.005,
                    damping: 0.9,
                    iterations: 8,
                    max_dt: 1.0 / 30.0,
                },
                chains: vec![ClothChain {
                    anchor: [0.0, 0.0, 0.0],
                    dir: [1.0, 0.0, 0.0],
                    seg_len: 5.0,
                    segments: 4,
                }],
                binds: vec![
                    ClothBind {
                        v: 0,
                        c: 0,
                        k: 0,
                        f: 0.0,
                    }, // at the anchor
                    ClothBind {
                        v: 1,
                        c: 0,
                        k: 2,
                        f: 0.0,
                    },
                    ClothBind {
                        v: 2,
                        c: 0,
                        k: 3,
                        f: 1.0,
                    }, // the free tip
                ],
            }],
        };
        let verts: Vec<Vertex> = orig.iter().map(|p| vtx(*p)).collect();
        (ClothSim::build(&cloth, &verts, &[], &[bone("a")]), orig)
    }

    fn drive(
        sim: &mut ClothSim,
        palette: &[Mat4],
        orig: &[[f32; 3]],
        frames: usize,
    ) -> Vec<SkinnedVertex> {
        let mut skinned: Vec<SkinnedVertex> = orig.iter().map(|p| sv(*p)).collect();
        for _ in 0..frames {
            sim.update(palette, 1.0 / 60.0, &mut skinned);
        }
        skinned
    }

    /// The whole region DRAPES off its modelled shape: at the bind pose the tip vertex falls
    /// well below its rest height under gravity, while the anchor-bound vertex stays put.
    #[test]
    fn region_drapes_off_the_rest_shape() {
        let (mut sim, orig) = one_region_sim();
        let out = drive(&mut sim, &[Mat4::IDENTITY], &orig, 300);
        assert!(
            Vec3::from(out[0].position).length() < 1e-2,
            "the anchor-bound vert stays at the anchor"
        );
        assert!(
            out[2].position[2] < -5.0,
            "the tip must drape well below its rest height (z {})",
            out[2].position[2]
        );
        assert!(
            out.iter().all(|s| Vec3::from(s.position).is_finite()),
            "cloth must stay finite"
        );
    }

    /// Moving the anchor bone carries the whole hang with it; the anchor-bound vert tracks
    /// the bone exactly, and everything stays bounded.
    #[test]
    fn anchor_move_carries_the_hang() {
        let (mut sim, orig) = one_region_sim();
        let out = drive(
            &mut sim,
            &[Mat4::from_translation(Vec3::new(40.0, 0.0, 0.0))],
            &orig,
            40,
        );
        assert!(
            (Vec3::from(out[0].position) - Vec3::new(40.0, 0.0, 0.0)).length() < 1e-2,
            "the anchor-bound vert must track the bone (got {:?})",
            out[0].position
        );
        assert!(
            out.iter().all(
                |s| Vec3::from(s.position).is_finite() && Vec3::from(s.position).length() < 1e4
            ),
            "cloth must stay bounded"
        );
    }

    /// A region of `chains` parallel hangs along -z, `span` apart, with a vertex sitting ON each
    /// chain and one exactly MIDWAY between the first two. Returns the sim and the rest positions.
    fn comb_sim(chains: usize, span: f32, mid: bool) -> (ClothSim, Vec<[f32; 3]>) {
        let (seg_len, segments) = (5.0_f32, 4_u32);
        let z = -10.0_f32; // k = 2, f = 0 on every chain
        let mut orig: Vec<[f32; 3]> = (0..chains).map(|c| [span * c as f32, 0.0, z]).collect();
        let mut binds: Vec<ClothBind> = (0..chains)
            .map(|c| ClothBind {
                v: c as u32,
                c: c as u32,
                k: 2,
                f: 0.0,
            })
            .collect();
        if mid {
            orig.push([span * 0.5, 0.0, z]);
            binds.push(ClothBind {
                v: chains as u32,
                c: 0,
                k: 2,
                f: 0.0,
            });
        }
        let cloth = Cloth {
            regions: vec![ClothRegion {
                name: "panel".into(),
                anchor_bone: "a".into(),
                tag: RegionTag::Hair,
                verts: Vec::new(),
                chain_count: chains as u32,
                params: ClothParams {
                    gravity: [0.0, 0.0, -600.0],
                    stiffness: 0.015,
                    damping: 0.9,
                    iterations: 8,
                    max_dt: 1.0 / 30.0,
                },
                chains: (0..chains)
                    .map(|c| ClothChain {
                        anchor: [span * c as f32, 0.0, 0.0],
                        dir: [0.0, 0.0, -1.0],
                        seg_len,
                        segments,
                    })
                    .collect(),
                binds,
            }],
        };
        let verts: Vec<Vertex> = orig.iter().map(|p| vtx(*p)).collect();
        (ClothSim::build(&cloth, &verts, &[], &[bone("a")]), orig)
    }

    /// THE COMB GATE (EC30FD2E): a vertex midway between two chains lands MIDWAY between what the
    /// two chains put it at — so a wide panel reads as a sheet instead of swinging rigidly off one
    /// chain. Driven ALONG the comb (+x, the axis the chains are laid across) so the segment bend
    /// actually tilts a cross-panel offset; the control (the same region with the second chain
    /// removed, so the blend cannot run) must land somewhere else.
    #[test]
    fn a_vertex_midway_between_two_chains_lands_midway() {
        let swing = Mat4::from_translation(Vec3::new(60.0, 0.0, 0.0));
        let (mut sim, orig) = comb_sim(2, 20.0, true);
        let out = drive(&mut sim, &[swing], &orig, 12);
        let (a, b, mid) = (
            Vec3::from(out[0].position),
            Vec3::from(out[1].position),
            Vec3::from(out[2].position),
        );
        assert!(
            (mid - (a + b) * 0.5).length() < 1e-3,
            "the midway vertex must be the midpoint of its two chains' answers: {mid} vs {}",
            (a + b) * 0.5
        );
        // The control: one chain only, so the same vertex rides chain 0 alone.
        let (mut one, orig1) = comb_sim(1, 20.0, true);
        let solo = drive(&mut one, &[swing], &orig1, 12);
        let solo_mid = Vec3::from(solo[1].position);
        assert!(
            (mid - solo_mid).length() > 0.1,
            "the blend must actually change the answer (single-chain {solo_mid}, combed {mid})"
        );
    }

    /// The rig's OWN `collision` block is where the body capsules come from: they resolve by bone
    /// NAME into bind space, a box is not a capsule and is skipped, and a rig from before the
    /// capsule bake (no `collision` at all) resolves to none and collides with nothing.
    #[test]
    fn capsules_resolve_from_the_rigs_own_collision_block() {
        use crate::format::{CollisionRole, CollisionShape, CollisionVolume};
        let vol = |bone: &str, shape| CollisionVolume {
            name: String::new(),
            bone: bone.into(),
            shape,
            role: CollisionRole::Physics,
        };
        let mut sim = comb_sim(1, 20.0, false).0;
        sim.set_capsules(&[], &[bone("a")]);
        assert!(sim.capsules().is_empty(), "no collision block, no capsules");
        sim.set_capsules(
            &[
                vol(
                    "a",
                    CollisionShape::Capsule {
                        a: [0.0, 0.0, 0.0],
                        b: [0.0, 0.0, -20.0],
                        radius: 8.0,
                    },
                ),
                vol(
                    "nobody",
                    CollisionShape::Capsule {
                        a: [0.0; 3],
                        b: [1.0; 3],
                        radius: 1.0,
                    },
                ),
                vol(
                    "a",
                    CollisionShape::Box {
                        center: [0.0; 3],
                        half_extents: [1.0; 3],
                        rotation: [0.0, 0.0, 0.0, 1.0],
                    },
                ),
            ],
            &[bone("a")],
        );
        let caps: Vec<_> = sim.capsules().to_vec();
        assert_eq!(
            caps.len(),
            1,
            "one capsule: an unknown bone and a box are not"
        );
        assert_eq!(caps[0].bone, 0);
        assert_eq!(caps[0].radius, 8.0);
        assert!((caps[0].b - Vec3::new(0.0, 0.0, -20.0)).length() < 1e-4);
        // The free nodes of a straight -z hang all start INSIDE that capsule, which is what makes
        // the runtime's push (flicker-rigview, over `flicker_mechanics::collision`) do work.
        let radius = caps[0].radius;
        sim.step(&[Mat4::IDENTITY], 1.0 / 60.0);
        let mut seen = 0;
        sim.push_free_nodes(|p| {
            if p.truncate().length() < radius {
                seen += 1;
            }
            p
        });
        assert!(seen > 0, "the hang must start inside the capsule");
    }

    /// SETTLING IS IDEMPOTENT: a poster steps a fixed number of times and uploads once, so two
    /// consecutive settles of the same pose must produce the SAME vertices — otherwise a still
    /// image shimmers every time its surface is marked dirty.
    #[test]
    fn two_consecutive_poster_settles_are_identical() {
        let (mut sim, orig) = comb_sim(3, 15.0, true);
        let palette = [Mat4::from_translation(Vec3::new(5.0, 0.0, 0.0))];
        let settle = |sim: &mut ClothSim| {
            let mut out: Vec<SkinnedVertex> = orig.iter().map(|p| sv(*p)).collect();
            for _ in 0..SETTLE_STEPS {
                sim.step(&palette, 1.0 / 60.0);
            }
            sim.place(&mut out);
            out
        };
        let first = settle(&mut sim);
        let second = settle(&mut sim);
        for (a, b) in first.iter().zip(second.iter()) {
            // A settled chain is CONVERGED, not frozen: PBD keeps creeping by a hair. The gate is
            // that the creep is invisible — well under a thousandth of a centimetre.
            let d = (Vec3::from(a.position) - Vec3::from(b.position)).length();
            assert!(d < 1e-3, "a settled poster must not move, drifted {d}");
        }
    }

    /// THE BUDGET (spec 6C46CAB9): the Traveler Duster's shape — 10 regions, 30 chains, ~300 free
    /// nodes, 4 000 cloth vertices — must cost WELL under a millisecond of CPU per frame, or the
    /// whole design is unaffordable. Measured and PRINTED (`cargo test -- --nocapture`).
    #[test]
    fn a_duster_sized_cloth_costs_well_under_a_millisecond_per_frame() {
        const REGIONS: usize = 10;
        const CHAINS: usize = 3;
        const PER_CHAIN: usize = 134; // 4 020 bound verts over 30 chains
        let mut verts: Vec<Vertex> = Vec::new();
        let regions: Vec<ClothRegion> = (0..REGIONS)
            .map(|ri| {
                let mut binds = Vec::new();
                for c in 0..CHAINS {
                    for i in 0..PER_CHAIN {
                        let k = (i % 5).min(4) as u32;
                        binds.push(ClothBind {
                            v: verts.len() as u32,
                            c: c as u32,
                            k,
                            f: 0.5,
                        });
                        verts.push(vtx([
                            20.0 * c as f32 + (i % 7) as f32,
                            ri as f32,
                            -5.0 * k as f32,
                        ]));
                    }
                }
                ClothRegion {
                    name: format!("panel_{ri}"),
                    anchor_bone: "a".into(),
                    tag: RegionTag::Hair,
                    verts: Vec::new(),
                    chain_count: CHAINS as u32,
                    params: ClothParams::default(),
                    chains: (0..CHAINS)
                        .map(|c| ClothChain {
                            anchor: [20.0 * c as f32, ri as f32, 0.0],
                            dir: [0.0, 0.0, -1.0],
                            seg_len: 5.0,
                            segments: 5,
                        })
                        .collect(),
                    binds,
                }
            })
            .collect();
        let cloth = Cloth { regions };
        let mut sim = ClothSim::build(&cloth, &verts, &[], &[bone("a")]);
        let mut skinned: Vec<SkinnedVertex> = verts.iter().map(|v| sv(v.p)).collect();
        let palette = [Mat4::from_translation(Vec3::new(1.0, 0.0, 0.0))];
        // Warm the buffers (the first frame sizes them; every later one reuses them).
        sim.update(&palette, 1.0 / 60.0, &mut skinned);
        const FRAMES: usize = 200;
        let t0 = std::time::Instant::now();
        for _ in 0..FRAMES {
            sim.update(&palette, 1.0 / 60.0, &mut skinned);
        }
        let per_frame = t0.elapsed().as_secs_f64() * 1000.0 / FRAMES as f64;
        let floor = machine_floor_ms();
        eprintln!(
            "cloth budget: {} regions / {} chains / {} bound verts = {:.4} ms per frame; the \
             machine floor {:.4} ms; {:.2} floors",
            REGIONS,
            REGIONS * CHAINS,
            verts.len(),
            per_frame,
            floor,
            per_frame / floor
        );
        assert!(
            per_frame < COMB_BUDGET_FLOORS * floor,
            "a duster-sized cloth must cost under {COMB_BUDGET_FLOORS} machine floors \
             ({floor:.4} ms each), measured {per_frame:.4} ms"
        );
    }

    // ── THE SHEET ──────────────────────────────────────────────────────────────────────────

    /// A `rows` × `cols` panel lying flat in the xy plane (z = 0), `s` apart, two triangles a
    /// quad: row 0 is the BODY beside the panel (not a member), every other row the `Cloth`
    /// region, so row 1 is the seam. Returns the cloth, its vertices and its triangles.
    /// THE MACHINE'S OWN SPEED, for the budget gates: the milliseconds this core takes to run a
    /// projection-shaped bare loop — the arithmetic of one distance constraint (two positions, a
    /// difference, a length, a correction applied both ways) over [`FLOOR_PROJECTIONS`] pairs
    /// held in cache. A frame budget read in FLOORS instead of milliseconds holds a shared CI
    /// runner at half a desk's speed to the same EFFICIENCY, not the same wall clock (the
    /// skirt gate, 2.2 ms on the desk it was written at, read 5.1 ms on the macOS runner and
    /// failed, 2026-10-09). The least of [`FLOOR_PROBES`] runs, so a scheduler hiccup during a
    /// probe cannot shrink the floor. 0.30 ms on the desk it was written at (1.5 ns a
    /// projection). Measured and PRINTED beside the frame it budgets.
    const FLOOR_PROJECTIONS: usize = 200_000;
    const FLOOR_PROBES: usize = 5;
    fn machine_floor_ms() -> f64 {
        const PAIRS: usize = 1000;
        let mut a: Vec<Vec3> = (0..PAIRS)
            .map(|i| Vec3::new(i as f32 * 0.01, 0.0, 0.0))
            .collect();
        let mut b: Vec<Vec3> = a.iter().map(|p| *p + Vec3::new(0.3, 1.1, 0.0)).collect();
        let mut least = f64::INFINITY;
        for _ in 0..FLOOR_PROBES {
            let t0 = std::time::Instant::now();
            for _ in 0..FLOOR_PROJECTIONS / PAIRS {
                for i in 0..PAIRS {
                    let d = b[i] - a[i];
                    let len = d.length();
                    let corr = d * (0.5 * (len - 1.0) / len.max(1e-6));
                    a[i] += corr;
                    b[i] -= corr;
                }
            }
            least = least.min(t0.elapsed().as_secs_f64() * 1000.0);
            std::hint::black_box((&a, &b));
        }
        least
    }

    /// A duster's comb (ten regions of three five-segment chains over 4 000 bound vertices) and a
    /// skirt's sheet, each in machine floors: 0.18 and 8.4 when written, held to the same
    /// headroom the millisecond gates had (1 ms over 0.055; 4 ms over 2.2).
    const COMB_BUDGET_FLOORS: f64 = 3.0;
    const SHEET_BUDGET_FLOORS: f64 = 15.0;

    fn panel(rows: usize, cols: usize, s: f32, stiffness: f32) -> (Cloth, Vec<Vertex>, Vec<u32>) {
        let mut verts = Vec::new();
        for i in 0..rows {
            for j in 0..cols {
                verts.push(vtx([j as f32 * s, i as f32 * s, 0.0]));
            }
        }
        let at = |i: usize, j: usize| (i * cols + j) as u32;
        let mut tris = Vec::new();
        for i in 0..rows - 1 {
            for j in 0..cols - 1 {
                tris.extend_from_slice(&[at(i, j), at(i, j + 1), at(i + 1, j)]);
                tris.extend_from_slice(&[at(i, j + 1), at(i + 1, j + 1), at(i + 1, j)]);
            }
        }
        let members: Vec<u32> = (cols as u32..(rows * cols) as u32).collect();
        let cloth = Cloth {
            regions: vec![ClothRegion {
                name: "skirt".into(),
                anchor_bone: "a".into(),
                tag: RegionTag::Cloth,
                verts: members,
                chain_count: 1,
                params: ClothParams {
                    gravity: [0.0, 0.0, -600.0],
                    stiffness,
                    damping: 0.9,
                    iterations: 8,
                    max_dt: 1.0 / 30.0,
                },
                chains: Vec::new(),
                binds: Vec::new(),
            }],
        };
        (cloth, verts, tris)
    }

    fn settle_panel(sim: &mut ClothSim, verts: &[Vertex], palette: &[Mat4]) -> Vec<SkinnedVertex> {
        let mut out: Vec<SkinnedVertex> = verts.iter().map(|v| sv(v.p)).collect();
        for _ in 0..SETTLE_STEPS {
            sim.step(palette, 1.0 / 60.0);
        }
        sim.place(&mut out);
        out
    }

    /// THE SHEET HANGS FROM ITS SEAM (ruling 82EDC071): a flat panel sewn to the body along one
    /// row falls under gravity and hangs down from that row — the seam stays on the skin to the
    /// hair, the far row ends well below, every edge keeps its bind length within 3 %, and the
    /// body row beside it is never touched.
    #[test]
    fn a_sheet_hangs_from_its_seam_and_keeps_its_edges() {
        let (rows, cols, s) = (10, 6, 5.0);
        let (cloth, verts, tris) = panel(rows, cols, s, 0.015);
        let mut sim = ClothSim::build(&cloth, &verts, &tris, &[bone("a")]);
        let out = settle_panel(&mut sim, &verts, &[Mat4::IDENTITY]);
        let p = |i: usize, j: usize| Vec3::from(out[i * cols + j].position);
        for j in 0..cols {
            assert!(
                (p(0, j) - Vec3::from(verts[j].p)).length() < 1e-6,
                "the body row is not the sheet's"
            );
            assert!(
                (p(1, j) - Vec3::from(verts[cols + j].p)).length() < 1e-4,
                "the seam rides the skin: {:?}",
                p(1, j)
            );
        }
        let free = (rows - 2) as f32 * s;
        for j in 0..cols {
            assert!(
                p(rows - 1, j).z < -0.7 * free,
                "the far row hangs down: z {} for a {free} cm drop",
                p(rows - 1, j).z
            );
        }
        for t in tris.as_chunks::<3>().0 {
            for (a, b) in [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])] {
                let (a, b) = (a as usize, b as usize);
                if a < cols || b < cols {
                    continue;
                }
                let rest = Vec3::from(verts[a].p).distance(Vec3::from(verts[b].p));
                let now = Vec3::from(out[a].position).distance(Vec3::from(out[b].position));
                assert!(
                    (now - rest).abs() < 0.03 * rest,
                    "an edge keeps its length: {now:.2} vs {rest:.2}"
                );
            }
        }
        assert!(out.iter().all(|v| Vec3::from(v.position).is_finite()));
    }

    /// THE SHEET FOLLOWS THE BODY: when the anchor bone moves, the seam goes with it and the
    /// whole panel comes along — and a second settle at the same pose lands within a hair of the
    /// first (a poster never shimmers).
    #[test]
    fn a_sheet_follows_its_seam_and_settles_the_same_twice() {
        let (rows, cols, s) = (8, 5, 5.0);
        let (cloth, verts, tris) = panel(rows, cols, s, 0.015);
        let mut sim = ClothSim::build(&cloth, &verts, &tris, &[bone("a")]);
        let moved = [Mat4::from_translation(Vec3::new(40.0, 0.0, 10.0))];
        let first = settle_panel(&mut sim, &verts, &moved);
        for j in 0..cols {
            let seam = Vec3::from(first[cols + j].position);
            let want = Vec3::from(verts[cols + j].p) + Vec3::new(40.0, 0.0, 10.0);
            assert!(
                (seam - want).length() < 1e-4,
                "the seam rode the bone: {seam}"
            );
        }
        for v in &first[2 * cols..] {
            assert!(
                v.position[0] > 30.0,
                "the panel came along in x: {:?}",
                v.position
            );
        }
        let second = settle_panel(&mut sim, &verts, &moved);
        for (a, b) in first.iter().zip(&second) {
            // Converged, not frozen: the relaxation keeps creeping by a hair, and the gate is
            // that the creep is invisible — under a tenth of a millimetre.
            let d = (Vec3::from(a.position) - Vec3::from(b.position)).length();
            assert!(d < 1e-2, "a settled sheet must not move, drifted {d}");
        }
    }

    /// STIFFNESS IS THE FOLD'S RESISTANCE: the same hanging panel, its seam slid 30 cm sideways
    /// over a third of a second and then held, folds less when its bending stiffness is higher —
    /// the sharpest angle between consecutive rows down the panel's middle column, read after
    /// the slide while the panel swings (limp 83°, stiff 32° when this was written).
    #[test]
    fn a_stiffer_sheet_folds_less() {
        let (rows, cols, s) = (10, 5, 5.0);
        let fold = |stiffness: f32| -> f32 {
            let (cloth, verts, tris) = panel(rows, cols, s, stiffness);
            let mut sim = ClothSim::build(&cloth, &verts, &tris, &[bone("a")]);
            settle_panel(&mut sim, &verts, &[Mat4::IDENTITY]);
            let mut out: Vec<SkinnedVertex> = verts.iter().map(|v| sv(v.p)).collect();
            let mid = cols / 2;
            let mut worst = 0.0f32;
            for f in 0..60 {
                let x = 30.0 * ((f + 1) as f32 / 20.0).min(1.0);
                sim.step(
                    &[Mat4::from_translation(Vec3::new(x, 0.0, 0.0))],
                    1.0 / 60.0,
                );
                sim.place(&mut out);
                if f < 20 {
                    continue;
                }
                for i in 2..rows - 1 {
                    let a = Vec3::from(out[(i - 1) * cols + mid].position);
                    let b = Vec3::from(out[i * cols + mid].position);
                    let c = Vec3::from(out[(i + 1) * cols + mid].position);
                    let ang = (b - a)
                        .normalize_or_zero()
                        .dot((c - b).normalize_or_zero())
                        .clamp(-1.0, 1.0)
                        .acos()
                        .to_degrees();
                    worst = worst.max(ang);
                }
            }
            worst
        };
        let (limp, stiff) = (fold(0.0), fold(1.0));
        eprintln!("sheet fold after the slide: limp {limp:.1}°, stiff {stiff:.1}°");
        assert!(
            stiff < limp * 0.6,
            "a stiffer sheet folds less: limp {limp:.1}°, stiff {stiff:.1}°"
        );
    }

    /// AN ISLAND PINS ITSELF: a `Cloth` region with no body beside it (the whole mesh tagged)
    /// pins its members nearest the anchor bone's head, so it hangs from there instead of
    /// falling out of the world.
    #[test]
    fn an_island_sheet_hangs_from_its_anchor_bone() {
        let (rows, cols, s) = (8, 4, 5.0);
        let (mut cloth, verts, tris) = panel(rows, cols, s, 0.015);
        cloth.regions[0].verts = (0..(rows * cols) as u32).collect();
        let mut sim = ClothSim::build(&cloth, &verts, &tris, &[bone("a")]);
        let out = settle_panel(&mut sim, &verts, &[Mat4::IDENTITY]);
        // The bone's head is the origin, the panel's (0, 0) corner: that corner stays.
        let corner = Vec3::from(out[0].position);
        assert!(
            corner.length() < 1e-4,
            "the corner at the bone stays: {corner}"
        );
        let far = Vec3::from(out[(rows - 1) * cols + cols - 1].position);
        assert!(
            far.z < -10.0 && far.z > -200.0,
            "the far corner hangs, bounded: {far}"
        );
    }

    /// THE TAG PICKS THE SOLVER: a `Cloth` region is a sheet, a `Hair` region a comb, and a
    /// `Cloth` region of no chains is rigid — nothing simulated at all.
    #[test]
    fn the_tag_picks_the_solver() {
        let (cloth, verts, tris) = panel(4, 4, 5.0, 0.015);
        let sim = ClothSim::build(&cloth, &verts, &tris, &[bone("a")]);
        assert_eq!(
            (sim.sheets.len(), sim.regions.len()),
            (1, 0),
            "Cloth is a sheet"
        );
        let mut rigid = cloth.clone();
        rigid.regions[0].chain_count = 0;
        let sim = ClothSim::build(&rigid, &verts, &tris, &[bone("a")]);
        assert!(sim.is_empty(), "a Cloth region of no chains is rigid");
        let (comb, orig) = comb_sim(2, 20.0, false);
        assert_eq!(
            (comb.sheets.len(), comb.regions.len()),
            (0, 1),
            "Hair is a comb"
        );
        assert_eq!(orig.len(), 2);
    }

    /// FREE PARTICLES ARE THE RUNTIME'S TO PUSH: `push_free_nodes` hands every free particle of
    /// a sheet (and none of its pinned seam) to the body's collision, and what comes back is
    /// what `place` writes.
    #[test]
    fn a_sheets_free_particles_take_the_push() {
        let (rows, cols, s) = (6, 4, 5.0);
        let (cloth, verts, tris) = panel(rows, cols, s, 0.015);
        let mut sim = ClothSim::build(&cloth, &verts, &tris, &[bone("a")]);
        sim.step(&[Mat4::IDENTITY], 1.0 / 60.0);
        let mut seen = 0;
        sim.push_free_nodes(|p| {
            seen += 1;
            Vec3::new(p.x, p.y, p.z.max(-3.0))
        });
        assert_eq!(
            seen,
            (rows - 2) * cols,
            "every free particle, no pinned one"
        );
        let mut out: Vec<SkinnedVertex> = verts.iter().map(|v| sv(v.p)).collect();
        sim.place(&mut out);
        for v in &out[cols..] {
            assert!(
                v.position[2] >= -3.0 - 1e-5,
                "the push is what is placed: {:?}",
                v.position
            );
        }
    }

    /// THE BUDGET: a skirt's worth of sheet — 60 × 80 particles, ~14 000 stretch and ~14 000
    /// bending constraints over 8 passes, 220 000 projections a frame — costs a few milliseconds
    /// of one core (2.2–2.5 ms at this profile's opt-level 1 when written: ~11 ns a projection,
    /// some twenty times the comb per vertex — the price of a real sheet). Read in MACHINE
    /// FLOORS ([`machine_floor_ms`]): 8.4 when written, must stay under [`SHEET_BUDGET_FLOORS`].
    /// Measured and PRINTED.
    #[test]
    fn a_skirt_sized_sheet_costs_a_few_milliseconds_per_frame() {
        let (rows, cols) = (60, 80);
        let (cloth, verts, tris) = panel(rows, cols, 1.0, 0.015);
        let mut sim = ClothSim::build(&cloth, &verts, &tris, &[bone("a")]);
        let mut skinned: Vec<SkinnedVertex> = verts.iter().map(|v| sv(v.p)).collect();
        let palette = [Mat4::from_translation(Vec3::new(1.0, 0.0, 0.0))];
        sim.update(&palette, 1.0 / 60.0, &mut skinned);
        const FRAMES: usize = 100;
        let t0 = std::time::Instant::now();
        for _ in 0..FRAMES {
            sim.update(&palette, 1.0 / 60.0, &mut skinned);
        }
        let per_frame = t0.elapsed().as_secs_f64() * 1000.0 / FRAMES as f64;
        let floor = machine_floor_ms();
        eprintln!(
            "sheet budget: {} particles, {} stretch + {} bend constraints = {:.4} ms per frame; \
             the machine floor {:.4} ms; {:.2} floors",
            sim.sheets[0].pos.len(),
            sim.sheets[0].stretch.len(),
            sim.sheets[0].bend.len(),
            per_frame,
            floor,
            per_frame / floor
        );
        assert!(
            per_frame < SHEET_BUDGET_FLOORS * floor,
            "a skirt-sized sheet must cost under {SHEET_BUDGET_FLOORS} machine floors \
             ({floor:.4} ms each), measured {per_frame:.4} ms"
        );
    }

    /// The tool's JSON (mesh.cloth) round-trips through the serde types and builds a sim.
    #[test]
    fn parses_tool_json_and_builds() {
        let json = r#"{
          "vertices":[
            {"p":[0,0,0],"n":[0,1,0],"joints":[0,0,0,0],"weights":[1,0,0,0]},
            {"p":[1,0,-20],"n":[0,1,0],"joints":[0,0,0,0],"weights":[1,0,0,0]}
          ],
          "cloth":{"regions":[{
            "name":"sleeve_l","anchor_bone":"a","tag":"Hair",
            "params":{"gravity":[0,0,-500],"stiffness":0.06,"damping":0.9,"iterations":8,"max_dt":0.033},
            "chains":[{"anchor":[0,0,0],"dir":[0,0,-1],"seg_len":5,"segments":4}],
            "binds":[{"v":1,"c":0,"k":3,"f":1.0}]
          }]}
        }"#;
        let mesh: crate::format::Mesh = serde_json::from_str(json).expect("parse mesh+cloth");
        assert_eq!(mesh.cloth.regions.len(), 1);
        let sim = ClothSim::build(&mesh.cloth, &mesh.vertices, &[], &[bone("a")]);
        assert!(
            !sim.is_empty(),
            "the bound vert must produce a non-empty sim"
        );
    }
}
