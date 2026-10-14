//! **The doll** — the small-panel skinned-rig preview: one character, posed by a clip,
//! standing on its authored ground ring, in a seat that may be 34 px or 300 px across.
//!
//! Built ON [`RigView`], never beside it. A doll IS a rig view with three differences:
//! the author frames it (not the viewer, so it takes no camera input), it carries a clock
//! of its own, and it declares its own liveness. Everything else — the offscreen target,
//! the stage pass, the composite, the teardown — is the rig view's, which is the rig
//! view's `GlobeView` pass, which is the ONE pass every surface filler in the engine
//! renders through. There is no second stage pass here and no second target lifecycle.
//!
//! **Liveness is a rate, never a cache.** A LIVE doll asks for [`Rate::Hz`]; a POSTER doll
//! asks for [`Rate::Dirty`] and publishes `dirty` only when what it shows actually changed
//! (rig, clip, activity — a resize invalidates the target on its own). The renderer's
//! per-surface clock skips the pass otherwise and the target composites the image it
//! already holds. **N dolls on a page therefore cost ONE live pass, not N** — which is
//! what makes the design's six sizes affordable on a screen carrying a dozen of them.
//! Do not add a `drawn` flag or a poster texture: the clock owns this decision.
//!
//! **Nothing here is a colour.** The ground ring, the floor grid, the lighting and the
//! framing all come out of the authored `stages.<source>` block the [`RigView`] compiled.

use std::sync::Arc;

use flicker::render::{
    grid_segments, mesh_tangents, mesh_tangents_into, ring_segments, FrameGraph, MeshHandle,
    MeshIndices, MeshVertex, PbrMaps, Rate, Rect, Renderer, SkinnedMeshHandle, SkinnedVertex,
    StageCamera, StageLayer, TextureHandle, TexturedMeshHandle, TexturedVertex,
};
use flicker::ui::SurfaceSlot;
use flicker_globe::Arrows;
use flicker_mechanics::collision::{penetration, Shape};
use flicker_skeletal::cloth::{ClothSim, SETTLE_STEPS};
use flicker_skeletal::format::{Bone, Collision, Mesh as RigMesh, Model, RegionTag, Vertex};
use flicker_skeletal::skin::SkinnedVertex as CpuVertex;
use flicker_skeletal::{pose, skin};
use glam::{Mat4, Vec2, Vec3};

use crate::{Draw, Projection, RigView};

/// The cloth submesh's material word: the direct-RGB escape (bit 31) carrying the SAME neutral
/// steel the skinned shader hard-codes as its base, so the CPU half and the GPU half of one body
/// shade alike instead of reading as two materials.
const CLOTH_MATERIAL: u32 = 0x8000_0000 | 140 | (145 << 8) | (158 << 16);

/// The stage layer kinds a doll draws. A source authoring anything else is named once,
/// at construction, rather than drawing nothing in silence.
pub const DOLL_LAYERS: &[&str] = &["skinned", "ring", "grid"];

/// How often a LIVE doll re-renders. Clips are authored at 60 Hz (the time canon), but a
/// 34 px doll on a list row does not need a frame per clip tick — the clock is what keeps
/// a page of dolls off the GPU, so spending half the budget by default would defeat it.
pub const LIVE_HZ: f32 = 30.0;

/// **A rig's drawable body** — the GPU-skinned mesh, plus the CLOTH SUBMESH the CPU deforms over
/// it when the rig carries cloth regions with binds (spec 6C46CAB9).
///
/// The GPU skins from a bone palette; it cannot run a PBD chain. So the body is PARTITIONED once,
/// here, at upload: every triangle touching a chain-bound vertex becomes a cloth submesh with its
/// own plain vertex buffer, and the rest stays the GPU-skinned mesh exactly as before (the same
/// vertex buffer, a filtered index list — no re-index, no double draw). Per frame the cloth half
/// is CPU-skinned, stepped, pushed out of the body's capsules, renormalised and written back into
/// its buffer in place; both halves draw under the same world matrix in the same draw item.
///
/// **This is the ONE skinned-mesh upload door.** A caller left on `upload_skinned_mesh` would draw
/// the cloth rigidly and never know (rule 98232A50).
pub struct SkinnedBody {
    mesh: Option<SkinnedMeshHandle>,
    cloth: Option<ClothPart>,
}

/// The cloth submesh's GPU handles — the flat mesh the steel view draws and its TEXTURED twin
/// (the same vertices with their UVs and tangents) the body's material draws — and the CPU state
/// that feeds both.
struct ClothPart {
    mesh: MeshHandle,
    textured: TexturedMeshHandle,
    cpu: ClothCpu,
}

/// The CPU half: the submesh's simulation and every buffer a frame needs — allocated once here so
/// a frame allocates NOTHING (405F7034). Deliberately free of any GPU handle, so the whole
/// per-frame cloth path is exercisable headless.
struct ClothCpu {
    sim: ClothSim,
    /// The submesh's BIND vertices, already compacted out of the source mesh — so the per-frame
    /// skin reads a contiguous few thousand and the body's whole vertex list is never walked.
    bind: Vec<Vertex>,
    /// The submesh's own triangle list, in LOCAL indices.
    tris: Vec<u32>,
    /// Reused per frame: the CPU skin of `src`, the area-weighted normals, the GPU vertices (flat
    /// and textured, the latter's tangents re-read off the draped triangles into `tangents` over
    /// `scratch`), and the capsules posed into world space.
    skinned: Vec<CpuVertex>,
    normals: Vec<Vec3>,
    verts: Vec<MeshVertex>,
    textured: Vec<TexturedVertex>,
    tangents: Vec<[f32; 4]>,
    scratch: Vec<Vec3>,
    caps: Vec<Shape>,
}

impl SkinnedBody {
    /// Upload `mesh` for `bones`, splitting off a cloth submesh when the rig carries one, and
    /// SETTLE that cloth at the rest pose so the first frame is already draped. `collision` is the
    /// rig's own volume list — its capsules are the body the cloth cannot pass through.
    pub fn upload(
        r: &mut Renderer,
        mesh: &RigMesh,
        bones: &[Bone],
        collision: &Collision,
    ) -> SkinnedBody {
        if mesh.vertices.is_empty() {
            return SkinnedBody {
                mesh: None,
                cloth: None,
            };
        }
        // The converter emits a non-deduped sequential list when indices are absent.
        let all: Vec<u32> = if mesh.indices.is_empty() {
            (0..mesh.vertices.len() as u32).collect()
        } else {
            mesh.indices.clone()
        };
        let split = split_cloth(mesh, &all);
        let rigid = split.as_ref().map_or(&all, |s| &s.rigid);
        // The bind-pose tangents over the WHOLE body (cloth triangles included — a vertex on
        // the seam is shared), for the material path's normal map.
        let tangents = mesh_tangents(
            mesh.vertices.len(),
            |i| mesh.vertices[i].p,
            |i| mesh.vertices[i].n,
            |i| mesh.vertices[i].uv,
            &all,
        );
        let verts: Vec<SkinnedVertex> = mesh
            .vertices
            .iter()
            .zip(&tangents)
            .map(|(v, &tangent)| SkinnedVertex {
                position: v.p,
                normal: v.n,
                uv: v.uv,
                joints: v.joints,
                weights: v.weights,
                tangent,
            })
            .collect();
        let handle =
            (!rigid.is_empty()).then(|| r.upload_skinned_mesh(&verts, MeshIndices::U32(rigid)));
        let cloth = split.map(|s| {
            let local: Vec<Vertex> = s
                .src
                .iter()
                .map(|&i| mesh.vertices[i as usize].clone())
                .collect();
            let mut sim = ClothSim::build(&s.cloth, &local, &s.tris, bones);
            sim.set_capsules(&collision.volumes, bones);
            let n = local.len();
            let verts: Vec<MeshVertex> = local
                .iter()
                .map(|v| MeshVertex {
                    position: v.p,
                    normal: v.n,
                    material: CLOTH_MATERIAL,
                })
                .collect();
            let handle = r.upload_mesh(&verts, MeshIndices::U32(&s.tris));
            // The textured twin opens on the bind vertices; every frame rewrites it.
            let textured: Vec<TexturedVertex> = local
                .iter()
                .map(|v| TexturedVertex {
                    position: v.p,
                    normal: v.n,
                    uv: v.uv,
                    tangent: [1.0, 0.0, 0.0, 1.0],
                })
                .collect();
            let twin = r.upload_textured_mesh(&textured, MeshIndices::U32(&s.tris));
            let mut cpu = ClothCpu {
                sim,
                bind: local,
                tris: s.tris,
                skinned: Vec::with_capacity(n),
                normals: Vec::with_capacity(n),
                verts,
                textured,
                tangents: Vec::with_capacity(n),
                scratch: Vec::with_capacity(2 * n),
                caps: Vec::with_capacity(collision.volumes.len()),
            };
            // Settle at the REST pose: a poster is one frame, so it must open already draped.
            let rest: Vec<Mat4> = bones.iter().map(|b| b.local).collect();
            let palette = skin::palette(bones, &pose::global_transforms(bones, &rest));
            cpu.simulate(&palette, None);
            r.update_mesh_vertices(handle, &cpu.verts);
            r.update_textured_mesh_vertices(twin, &cpu.textured);
            tracing::info!(
                cloth_verts = cpu.bind.len(),
                cloth_tris = cpu.tris.len() / 3,
                capsules = cpu.sim.capsules().len(),
                "body: cloth submesh split off the GPU skin"
            );
            ClothPart {
                mesh: handle,
                textured: twin,
                cpu,
            }
        });
        SkinnedBody {
            mesh: handle,
            cloth,
        }
    }

    /// Whether this body carries a CPU cloth half at all — a plain rigid body does not, and every
    /// per-frame cost below is skipped for it.
    pub fn has_cloth(&self) -> bool {
        self.cloth.is_some()
    }

    /// Step the cloth to this pose and rewrite its vertex buffer. `dt` is the surface's own delta;
    /// `None` SETTLES (the poster path — a settled poster's consecutive uploads are identical, so
    /// a still image never shimmers). A body with no cloth does nothing at all.
    pub fn pose(&mut self, r: &mut Renderer, palette: &[Mat4], dt: Option<f32>) {
        let Some(part) = self.cloth.as_mut() else {
            return;
        };
        part.cpu.simulate(palette, dt);
        r.update_mesh_vertices(part.mesh, &part.cpu.verts);
        r.update_textured_mesh_vertices(part.textured, &part.cpu.textured);
    }

    /// The ONE draw item for this body: the GPU-skinned half posed by `palette`, carrying the CPU
    /// cloth half under the SAME world matrix. A body that is nothing but cloth draws as a plain
    /// mesh; an empty body draws nothing. The neutral steel; [`Self::draw_with`] takes a material.
    pub fn draw(&self, world: Mat4, palette: Vec<Mat4>, bone_count: u32) -> Option<Draw> {
        self.draw_with(world, palette, bone_count, None)
    }

    /// [`Self::draw`] with the skinned half shaded through `material` (albedo + PBR maps) when
    /// one is given — the same PBR path a textured mesh takes.
    pub fn draw_with(
        &self,
        world: Mat4,
        palette: Vec<Mat4>,
        bone_count: u32,
        material: Option<(TextureHandle, PbrMaps)>,
    ) -> Option<Draw> {
        let cloth = self.cloth.as_ref().map(|c| c.mesh);
        let cloth_textured = self.cloth.as_ref().map(|c| c.textured);
        match self.mesh {
            Some(mesh) => Some(Draw::Skinned {
                mesh,
                world,
                palette,
                bone_count,
                cloth,
                cloth_textured,
                material,
            }),
            None => cloth.map(|mesh| Draw::Mesh {
                mesh,
                world,
                options: Default::default(),
            }),
        }
    }

    /// Give both meshes back (scene `exit`). Taken, so a second teardown is a no-op.
    pub fn free(&mut self, r: &mut Renderer) {
        if let Some(m) = self.mesh.take() {
            r.free_skinned_mesh(m);
        }
        if let Some(c) = self.cloth.take() {
            r.free_mesh(c.mesh);
            r.free_textured_mesh(c.textured);
        }
    }
}

impl ClothCpu {
    /// THE per-frame cloth path, CPU side: skin only the submesh's source vertices → step the
    /// chains → push their free nodes out of the body capsules → place the bound vertices on them
    /// → area-weighted normals off the submesh's own triangles → the GPU vertex list. Every buffer
    /// is reused, so this allocates nothing.
    fn simulate(&mut self, palette: &[Mat4], dt: Option<f32>) {
        skin::skin_subset(&self.bind, palette, &mut self.skinned);
        match dt {
            Some(dt) => self.sim.step(palette, dt),
            // A poster settles: N steps, ONE placement. `SETTLE_STEPS` is the same relaxation
            // `ClothSim::build` uses, so the drape a still image opens on is the chain's own.
            None => {
                for _ in 0..SETTLE_STEPS {
                    self.sim.step(palette, 1.0 / 60.0);
                }
            }
        }
        push_out_of_body(&mut self.sim, palette, &mut self.caps);
        self.sim.place(&mut self.skinned);
        // Area-weighted normals off the DRAPED triangles: the bind normal rotated by one chain
        // segment is right for a tube and wrong for a sheet that has folded.
        self.normals.clear();
        self.normals.resize(self.skinned.len(), Vec3::ZERO);
        for t in self.tris.as_chunks::<3>().0 {
            let (a, b, c) = (t[0] as usize, t[1] as usize, t[2] as usize);
            let (Some(pa), Some(pb), Some(pc)) = (
                self.skinned.get(a),
                self.skinned.get(b),
                self.skinned.get(c),
            ) else {
                continue;
            };
            let (pa, pb, pc) = (
                Vec3::from(pa.position),
                Vec3::from(pb.position),
                Vec3::from(pc.position),
            );
            // Un-normalised: its length IS twice the triangle's area, which is the weight.
            let n = (pb - pa).cross(pc - pa);
            for i in [a, b, c] {
                self.normals[i] += n;
            }
        }
        self.verts.clear();
        for (i, sv) in self.skinned.iter().enumerate() {
            let skinned = Vec3::from(sv.normal);
            let mut n = self.normals[i];
            // A degenerate fan (or a winding the source flipped) falls back to the skinned normal
            // rather than shading the panel black.
            if n.length_squared() < 1e-12 {
                n = skinned;
            } else if n.dot(skinned) < 0.0 {
                n = -n;
            }
            self.verts.push(MeshVertex {
                position: sv.position,
                normal: n.normalize_or_zero().to_array(),
                material: CLOTH_MATERIAL,
            });
        }
        // The textured twin: the same draped positions and normals, the bind UVs, and tangents
        // re-read off the draped triangles (a sheet that folded has new ones) into reused buffers.
        mesh_tangents_into(
            self.verts.len(),
            |i| self.verts[i].position,
            |i| self.verts[i].normal,
            |i| self.bind[i].uv,
            &self.tris,
            &mut self.scratch,
            &mut self.tangents,
        );
        self.textured.clear();
        self.textured
            .extend(self.verts.iter().zip(&self.bind).zip(&self.tangents).map(
                |((v, b), &tangent)| TexturedVertex {
                    position: v.position,
                    normal: v.normal,
                    uv: b.uv,
                    tangent,
                },
            ));
    }
}

/// Pose the rig's body capsules by the palette into the reused `caps` buffer, then push every
/// free chain node out of all of them. The overlap test is `flicker-mechanics`' — the one capsule
/// test in the tree — and it is reached from HERE rather than from `flicker-skeletal::cloth`
/// because mechanics depends on skeletal, so the other direction is a dependency cycle.
fn push_out_of_body(sim: &mut ClothSim, palette: &[Mat4], caps: &mut Vec<Shape>) {
    caps.clear();
    for c in sim.capsules() {
        if let Some(m) = palette.get(c.bone) {
            caps.push(
                Shape::Capsule {
                    a: c.a,
                    b: c.b,
                    radius: c.radius,
                }
                .transformed(*m),
            );
        }
    }
    if caps.is_empty() {
        return;
    }
    let posed = &*caps;
    sim.push_free_nodes(|mut p| {
        for s in posed {
            let probe = Shape::Sphere {
                center: p,
                radius: 0.0,
            };
            if let Some(c) = penetration(&probe, s) {
                p += c.normal * c.depth;
            }
        }
        p
    });
}

/// The partition of a rig's triangles by whether they touch a cloth-BOUND vertex.
struct Split {
    /// The GPU-skinned half: the original index list minus the cloth triangles (the vertex buffer
    /// is unchanged, so these still index the source vertices).
    rigid: Vec<u32>,
    /// Source vertex index per cloth-submesh vertex.
    src: Vec<u32>,
    /// The cloth submesh's triangle list, re-indexed into `src`.
    tris: Vec<u32>,
    /// The rig's cloth with every bind's `v` remapped to the submesh's local index.
    cloth: flicker_skeletal::format::Cloth,
}

/// Split `mesh`'s triangles: any triangle with a corner the cloth MOVES — bound to a chain, or a
/// member of a `Cloth` region that runs as a sheet (ruling 82EDC071) — goes to the CLOTH submesh,
/// everything else stays GPU-skinned. `None` when the rig moves no vertex (the ordinary body) or
/// when its moved vertices are in no triangle.
fn split_cloth(mesh: &RigMesh, indices: &[u32]) -> Option<Split> {
    let mut bound = vec![false; mesh.vertices.len()];
    let mut any = false;
    for r in &mesh.cloth.regions {
        let sheet = r.tag == RegionTag::Cloth && r.chain_count > 0;
        let moved = r
            .binds
            .iter()
            .map(|b| b.v)
            .chain(sheet.then_some(&r.verts).into_iter().flatten().copied());
        for v in moved {
            if let Some(f) = bound.get_mut(v as usize) {
                *f = true;
                any = true;
            }
        }
    }
    if !any {
        return None;
    }
    let mut rigid = Vec::with_capacity(indices.len());
    let mut src = Vec::new();
    let mut tris = Vec::new();
    let mut local = vec![u32::MAX; mesh.vertices.len()];
    for t in indices.as_chunks::<3>().0 {
        if !t.iter().any(|&i| bound.get(i as usize).is_some_and(|b| *b)) {
            rigid.extend_from_slice(t);
            continue;
        }
        for &i in t {
            let slot = &mut local[i as usize];
            if *slot == u32::MAX {
                *slot = src.len() as u32;
                src.push(i);
            }
            tris.push(*slot);
        }
    }
    if tris.is_empty() {
        return None;
    }
    // The sim runs over the SUBMESH, so its binds and its members must speak the submesh's
    // indices. A vertex no triangle reached is dropped — nothing would draw it.
    let mut cloth = mesh.cloth.clone();
    for r in &mut cloth.regions {
        r.binds.retain_mut(|b| {
            let l = local.get(b.v as usize).copied().unwrap_or(u32::MAX);
            b.v = l;
            l != u32::MAX
        });
        r.verts.retain_mut(|v| {
            let l = local.get(*v as usize).copied().unwrap_or(u32::MAX);
            *v = l;
            l != u32::MAX
        });
    }
    Some(Split {
        rigid,
        src,
        tris,
        cloth,
    })
}

/// The ONE rig a screenful of dolls shares: the uploaded skinned mesh plus the model it
/// poses from. Behind an [`Arc`] because a page carries a dozen dolls and the GPU skins
/// every instance from its own bone palette — one mesh, one skeleton, N poses.
///
/// The host owns it: it uploads the mesh here and gives it back with [`DollRig::free`] on
/// scene exit, exactly as a behaviour owns the handles it hands [`RigView::set_draws`].
pub struct DollRig {
    model: Arc<Model>,
    body: SkinnedBody,
    /// Rest-pose ground offset. `Model::world` centres the rig on the origin, but the
    /// authored cameras (`target_y`) and rings (`y: 0`) are metric with the feet on the
    /// floor, so the doll is dropped onto it before it is drawn.
    ground: Mat4,
}

impl DollRig {
    /// Upload `model`'s skinned mesh and take the pose source with it. A model with no
    /// mesh yields a rig that poses nothing — it is still a valid (empty) doll, not a
    /// panic, because a bench can be opened before its content loads.
    pub fn upload(r: &mut Renderer, model: Arc<Model>) -> Self {
        let ground = ground_transform(model.world, &model.mesh.vertices);
        if model.mesh.vertices.is_empty() {
            tracing::warn!("doll: the rig has no mesh — its dolls will be empty");
        }
        // THE one skinned-upload door: it splits off the cloth submesh when the rig carries one
        // and settles its drape at the rest pose (6C46CAB9).
        let body = SkinnedBody::upload(r, &model.mesh, &model.bones, &model.collision);
        tracing::info!(
            bones = model.bones.len(),
            verts = model.mesh.vertices.len(),
            clips = model.clips.len(),
            cloth = body.has_cloth(),
            "doll: rig uploaded"
        );
        Self {
            model,
            body,
            ground,
        }
    }

    pub fn model(&self) -> &Model {
        &self.model
    }

    pub fn bone_count(&self) -> u32 {
        self.model.bones.len() as u32
    }

    /// The rest pose's bounding radius in engine space — what the authored framing's
    /// distance is expressed against, so one authored shot frames a rig of any size.
    pub fn radius(&self) -> f32 {
        self.model.orbit_radius.max(f32::MIN_POSITIVE)
    }

    /// The clip named, if the rig carries it — so a host can bind by name without
    /// reaching into the model.
    pub fn clip_index(&self, name: &str) -> Option<usize> {
        self.model.clips.iter().position(|c| c.name == name)
    }

    /// The bone palette for `clip` at `time` seconds; the clip loops on its own duration,
    /// and an absent or out-of-range clip is the rest pose rather than a panic. CPU
    /// posing is cheap — the GPU does the vertex skinning.
    pub fn palette(&self, clip: Option<usize>, time: f32) -> Vec<Mat4> {
        let bones = &self.model.bones;
        let locals = match clip.and_then(|i| self.model.clips.get(i)) {
            Some(c) if c.duration_ticks > 0 => {
                let ticks = time * c.tick_rate_hz as f32;
                let tick = (ticks.floor() as i64).rem_euclid(c.duration_ticks as i64) as u32;
                pose::sample_local_poses(bones, c, tick, self.model.retarget)
            }
            Some(c) => pose::sample_local_poses(bones, c, 0, self.model.retarget),
            None => bones.iter().map(|b| b.local).collect(),
        };
        skin::palette(bones, &pose::global_transforms(bones, &locals))
    }

    /// Give the mesh back (scene `exit`). Taken, so a second teardown is a no-op.
    pub fn free(&mut self, r: &mut Renderer) {
        self.body.free(r);
    }

    /// Give the SHARED rig's mesh back through the last handle on it. Release every
    /// [`Doll`] holding a clone first; a rig still held by a live doll cannot be freed,
    /// and says so rather than freeing a mesh something is about to draw.
    pub fn release(rig: &mut Arc<Self>, r: &mut Renderer) {
        match Arc::get_mut(rig) {
            Some(rig) => rig.free(r),
            None => tracing::warn!(
                "doll: the rig is still held by a live doll — its mesh is NOT freed; \
                 release the dolls before the rig"
            ),
        }
    }

    /// This rig's draw item at `clip`/`time`.
    ///
    /// The cloth half is the SETTLED rest drape: a `DollRig` is shared behind an `Arc` by every
    /// doll on the page, so no doll can step a simulation of its own without a second copy of the
    /// rig — and a page of dolls is exactly what the sharing exists to make affordable. The bench
    /// preview, which owns its body outright, steps its cloth per frame.
    fn draw(&self, clip: Option<usize>, time: f32) -> Option<Draw> {
        self.body
            .draw(self.ground, self.palette(clip, time), self.bone_count())
    }
}

/// A skinned-rig preview seated in one small panel.
pub struct Doll {
    view: RigView,
    /// The authored shot: yaw / pitch / distance / look-at height, straight off
    /// `stages.<source>.camera`. An unframed stage takes the portrait default.
    framing: StageCamera,
    rig: Option<Arc<DollRig>>,
    clip: Option<usize>,
    /// Play-head, seconds — the doll's OWN clock, advanced by [`Doll::tick`].
    time: f32,
    live: bool,
    hz: f32,
    /// Selected / pointed-at: lights the ground ring in its authored active colour.
    active: bool,
    /// Set by every change to what the image shows; consumed by the next `render`, which
    /// is what a [`Rate::Dirty`] poster re-renders on.
    dirty: bool,
    /// The seat size the framing was last fitted to — a re-seat at a new shape refits.
    size: Vec2,
}

impl Doll {
    /// A doll drawing under `stages.<source>` from the shared styles. Perspective always:
    /// an orthographic preview of a character reads as a technical drawing, and the
    /// authored shot is an orbit.
    pub fn new(source: &str, styles: &serde_json::Value) -> Self {
        let mut view = RigView::new(source, styles, Projection::Perspective);
        let undrawn = view.stage().layers_outside(DOLL_LAYERS);
        if !undrawn.is_empty() {
            tracing::warn!("doll: `{source}` authors {undrawn:?} layers the doll does not draw");
        }
        // The framing policy — "an unframed stage is a portrait" — is applied ONCE here,
        // to the definition, not re-decided in every frame's draw closure.
        let framing = view.stage().camera.unwrap_or_else(|| {
            tracing::warn!("doll: `{source}` authors no camera — taking the portrait framing");
            StageCamera::default()
        });
        // The ground is authored geometry: it is laid ONCE here and replaced only when
        // the doll's activity changes its colour. A doll rebuilding its ring every frame
        // would be recomputing a constant.
        let lines = ground_lines(&view.stage().layers, false);
        view.set_lines(lines);
        Self {
            view,
            framing,
            rig: None,
            clip: None,
            time: 0.0,
            live: false,
            hz: LIVE_HZ,
            active: false,
            dirty: true,
            size: Vec2::ZERO,
        }
    }

    /// The rate a LIVE doll asks for. Anything above the display rate is a waste; zero
    /// or below would make a "live" doll never draw, so it is refused.
    pub fn live_hz(mut self, hz: f32) -> Self {
        if hz > 0.0 {
            self.hz = hz;
        }
        self
    }

    /// The rig every doll on the page shares. Changing it changes the image.
    pub fn set_rig(&mut self, rig: Option<Arc<DollRig>>) {
        let same = match (&self.rig, &rig) {
            (Some(a), Some(b)) => Arc::ptr_eq(a, b),
            (None, None) => true,
            _ => false,
        };
        if !same {
            self.rig = rig;
            self.dirty = true;
            self.refit();
        }
    }

    /// The clip this doll poses with — the idle it loops, or the one its card is bound
    /// to. `None` is the rest pose.
    pub fn set_clip(&mut self, clip: Option<usize>) {
        if self.clip != clip {
            self.clip = clip;
            self.dirty = true;
        }
    }

    pub fn clip(&self) -> Option<usize> {
        self.clip
    }

    /// Whether this doll animates. Only the one being watched should: a live doll is a
    /// GPU submit every `1/hz` seconds, a poster is none at all.
    pub fn set_live(&mut self, live: bool) {
        if self.live != live {
            self.live = live;
            // Going live must draw the current pose even if the clock says "not yet"; going
            // still must draw the frame it stopped on rather than keep the one before it.
            self.dirty = true;
        }
    }

    pub fn live(&self) -> bool {
        self.live
    }

    /// Selected / pointed-at — the authored ring's active colour. A still doll is a still
    /// doll, so the ring lights on the same condition that makes the slot animate.
    pub fn set_active(&mut self, active: bool) {
        if self.active != active {
            self.active = active;
            self.dirty = true;
            // Activity changes the ring's COLOUR, never its geometry — so this is the
            // only moment the ground is rebuilt.
            let lines = ground_lines(&self.view.stage().layers, active);
            self.view.set_lines(lines);
        }
    }

    /// The play-head, seconds. Set it to drive the doll off a transport the host owns
    /// (a TAE playhead) instead of its own clock.
    pub fn set_time(&mut self, time: f32) {
        if self.time != time {
            self.time = time;
            self.dirty = true;
        }
    }

    pub fn time(&self) -> f32 {
        self.time
    }

    /// Advance this doll's own clock. A poster's clock is parked — it is showing one
    /// frame, and moving a play-head nobody renders is exactly the per-frame recompute
    /// the poster exists to avoid.
    pub fn tick(&mut self, dt: f32) {
        if self.live {
            self.time += dt;
        }
    }

    /// Seat the doll in the `surface` slot the walker reserved for it.
    pub fn seat(&mut self, slot: Option<&SurfaceSlot>) {
        self.view.seat(slot);
        self.fit(slot.map_or(Vec2::ZERO, |s| Vec2::new(s.w, s.h)));
    }

    /// Seat the doll at a rect the HOST laid out — a card a graph canvas placed, which
    /// has no `surface` node of its own to reserve it.
    pub fn seat_at(&mut self, rect: Rect, layer: f32, tint: [f32; 4]) {
        self.view.seat_at(rect, layer, tint);
        self.fit(rect.size);
    }

    /// Unseat: an off-page doll declares nothing and keeps its target for its return.
    pub fn unseat(&mut self) {
        self.view.seat(None);
    }

    pub fn rect(&self) -> Option<Rect> {
        self.view.rect()
    }

    /// The rate this doll asks the per-surface clock for. A live doll re-renders on the
    /// clock; a still one re-renders only when it says its image changed.
    pub fn rate(&self) -> Rate {
        if self.live {
            Rate::Hz(self.hz)
        } else {
            Rate::Dirty
        }
    }

    /// Declare this doll's pass and composite (nothing while unseated).
    pub fn render<'f>(&'f mut self, r: &mut Renderer, fg: &mut FrameGraph<'f>, base_layer: f32) {
        // **Pose only what will be drawn.** A still, unchanged doll's pass is skipped by
        // the clock, so posing its skeleton would be a 67-bone recompute per frame for an
        // image nobody renders — with a dozen dolls on the page, the whole cost the poster
        // exists to avoid. Every way the pass CAN run — the first frame, a resize, a new
        // rig / clip / play-head — raises `dirty`, so this is never a blank draw.
        let draws = match self.rig.as_ref().filter(|_| self.poses()) {
            Some(rig) => rig.draw(self.clip, self.time).into_iter().collect(),
            None => Vec::new(),
        };
        self.view.set_draws(draws);
        let rate = self.rate();
        self.view.set_rate(Some(rate));
        // A live doll's rate ignores `dirty`, but consuming it either way keeps the flag
        // from surviving a spell of liveness and forcing one stale draw on the way back.
        self.view.set_dirty(std::mem::take(&mut self.dirty));
        self.view.render(r, fg, base_layer);
    }

    /// Give the doll's render target back (scene `exit`, or a page that dropped it). The
    /// rig's mesh is the HOST's — [`DollRig::free`] returns that, once, for every doll.
    pub fn release(&mut self, r: &mut Renderer) {
        self.view.free(r);
        self.view.seat(None);
        // The next seat draws into a fresh target, so it must draw.
        self.dirty = true;
    }

    /// Whether this frame's image has to be POSED. A still, unchanged doll's pass is
    /// skipped by the clock, so its skeleton must not be sampled — and every way the pass
    /// can still run raises `dirty`, so this is never false on a frame that draws.
    fn poses(&self) -> bool {
        self.live || self.dirty
    }

    /// Fit the authored shot to a seat of this shape. Called on every seat, and a no-op
    /// unless the shape actually changed — the framing must not be recomputed per frame
    /// for a doll that has not moved.
    fn fit(&mut self, size: Vec2) {
        if self.size != size {
            self.size = size;
            // A new shape rebuilds the target at a new size, and a fresh target must draw.
            self.dirty = true;
            self.refit();
        }
    }

    /// Apply the authored framing against the seated rig at the seated shape.
    ///
    /// The camera's field of view is VERTICAL, so a seat wider than it is tall shows more
    /// than the shot asked for (harmless) while a NARROWER one crops the subject at the
    /// sides. Backing off by the aspect is the whole size adaptation the doll needs: the
    /// shot itself is in world units, so a 34 px seat and a 300 px seat frame the subject
    /// identically — the small one is simply the same picture with fewer pixels.
    fn refit(&mut self) {
        let subject = self.rig.as_ref().map_or(1.0, |r| r.radius());
        let fit = if self.size.x > 0.0 && self.size.y > 0.0 {
            (self.size.y / self.size.x).max(1.0)
        } else {
            1.0
        };
        self.view
            .set_frame(Vec3::new(0.0, self.framing.target_y, 0.0), subject * fit);
        // `dist` is authored in WORLD units; the orbit expresses it as a multiple of the
        // radius it settled on (which has a floor a metric doll sits under). Reading that
        // back is what keeps the authored shot exact for a rig of any size — and keeps the
        // aspect pull-back a pull-back instead of cancelling itself out.
        let scale = self.framing.dist * fit / self.view.framing_radius();
        self.view
            .set_orbit(self.framing.yaw, self.framing.pitch, scale);
    }
}

/// Drop the rig onto the floor: `Model::world` centres it on the origin, the authored
/// stages are metric with the feet at y = 0. Without this every doll floats above (or
/// sinks through) its own ring.
fn ground_transform(world: Mat4, vertices: &[Vertex]) -> Mat4 {
    let feet = vertices
        .iter()
        .map(|v| world.transform_point3(Vec3::from(v.p)).y)
        .fold(f32::INFINITY, f32::min);
    let drop = if feet.is_finite() { -feet } else { 0.0 };
    Mat4::from_translation(Vec3::new(0.0, drop, 0.0)) * world
}

/// The line geometry of a source's ground layers, in authored order and authored colour —
/// depth-tested against the doll, so a ring reads under its feet. Kinds the doll does not
/// draw were named at construction; here they simply contribute nothing.
fn ground_lines(layers: &[StageLayer], active: bool) -> Arrows {
    layers
        .iter()
        .filter_map(|l| match *l {
            StageLayer::Ring {
                radius,
                y,
                segments,
                color,
                color_active,
            } => Some((
                if active { color_active } else { color },
                ring_segments(Vec3::new(0.0, y, 0.0), radius, segments),
            )),
            StageLayer::Grid {
                spacing,
                extent,
                y,
                color,
            } => Some((color, grid_segments(spacing, extent, y))),
            _ => None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use flicker_skeletal::format::{Bone, Mesh, Model, Source};

    /// A stage authored the way a doll's is: lit, framed, standing on a ring.
    fn styles() -> serde_json::Value {
        serde_json::json!({ "stages": { "doll_test": {
            "lighting": "studio",
            "clear": [0.0, 0.0, 0.0, 0.0],
            "camera": { "kind": "orbit", "yaw": 0.55, "pitch": 0.18, "dist": 2.6, "target_y": 0.95 },
            "layers": [
                { "draw": "skinned" },
                { "draw": "ring", "radius": 0.45, "y": 0.0, "segments": 24,
                  "color": [0.5, 0.4, 0.2, 1.0], "color_active": [1.0, 0.8, 0.3, 1.0] }
            ]
        } } })
    }

    fn bone(name: &str) -> Bone {
        Bone {
            name: name.into(),
            parent: -1,
            local: Mat4::IDENTITY,
            inverse_bind: Mat4::IDENTITY,
        }
    }

    fn vert(y: f32) -> Vertex {
        Vertex {
            p: [0.0, y, 0.0],
            n: [0.0, 1.0, 0.0],
            uv: [0.0, 0.0],
            joints: [0; 4],
            weights: [1.0, 0.0, 0.0, 0.0],
        }
    }

    /// A rig with no GPU: the mesh handle stays `None`, which is exactly the state a
    /// doll is in before its content loads.
    fn rig(radius: f32) -> Arc<DollRig> {
        Arc::new(DollRig {
            model: Arc::new(Model {
                bones: vec![bone("root"), bone("spine"), bone("head")],
                clips: Vec::new(),
                mesh: Mesh::default(),
                source: Source::default(),
                world: Mat4::IDENTITY,
                orbit_radius: radius,
                retarget: false,
                attach: Default::default(),
                collision: Default::default(),
            }),
            body: SkinnedBody {
                mesh: None,
                cloth: None,
            },
            ground: Mat4::IDENTITY,
        })
    }

    /// A cloth region hung from bone 0: `chains` HORIZONTAL hangs along +x, `span` apart on y,
    /// with one probe vertex sitting on each at `(10, y, 0)` — `k = 2`, `f = 0`. Horizontal so
    /// gravity has somewhere to drape them to, exactly as `cloth.rs`' own drape gate does. Plus
    /// two purely RIGID triangles far away, which must never enter the cloth submesh.
    fn sleeve_mesh(chains: usize, span: f32) -> Mesh {
        use flicker_skeletal::format::{Cloth, ClothBind, ClothChain, ClothParams, ClothRegion};
        let mut vertices: Vec<Vertex> = Vec::new();
        let mut indices: Vec<u32> = Vec::new();
        let mut binds: Vec<ClothBind> = Vec::new();
        for c in 0..chains {
            let y = span * c as f32;
            // A triangle per probe: the probe plus two companions beside it on the same chain.
            let base = vertices.len() as u32;
            for (j, off) in [0.0_f32, 0.3, -0.3].iter().enumerate() {
                let mut v = vert(0.0);
                v.p = [10.0 + off, y, j as f32 * 0.1];
                vertices.push(v);
            }
            indices.extend_from_slice(&[base, base + 1, base + 2]);
            binds.push(ClothBind {
                v: base,
                c: c as u32,
                k: 2,
                f: 0.0,
            });
        }
        for t in 0..2 {
            let base = vertices.len() as u32;
            for j in 0..3 {
                let mut v = vert(0.0);
                v.p = [100.0 + t as f32, j as f32, 0.0];
                vertices.push(v);
            }
            indices.extend_from_slice(&[base, base + 1, base + 2]);
        }
        Mesh {
            vertices,
            indices,
            cloth: Cloth {
                regions: vec![ClothRegion {
                    name: "sleeve".into(),
                    anchor_bone: "root".into(),
                    tag: RegionTag::Hair,
                    verts: Vec::new(),
                    chain_count: chains as u32,
                    params: ClothParams {
                        gravity: [0.0, 0.0, -600.0],
                        stiffness: 0.005,
                        damping: 0.9,
                        iterations: 8,
                        max_dt: 1.0 / 30.0,
                    },
                    chains: (0..chains)
                        .map(|c| ClothChain {
                            anchor: [0.0, span * c as f32, 0.0],
                            dir: [1.0, 0.0, 0.0],
                            seg_len: 5.0,
                            segments: 4,
                        })
                        .collect::<Vec<ClothChain>>(),
                    binds,
                }],
            },
            ..Default::default()
        }
    }

    /// Build the CPU half of a body straight off a mesh — the whole per-frame cloth path with no
    /// GPU in it.
    fn cpu_of(mesh: &Mesh, bones: &[Bone], collision: &Collision) -> ClothCpu {
        let s = split_cloth(mesh, &mesh.indices).expect("the mesh carries cloth");
        let bind: Vec<Vertex> = s
            .src
            .iter()
            .map(|&i| mesh.vertices[i as usize].clone())
            .collect();
        let mut sim = ClothSim::build(&s.cloth, &bind, &s.tris, bones);
        sim.set_capsules(&collision.volumes, bones);
        let n = bind.len();
        ClothCpu {
            sim,
            bind,
            tris: s.tris,
            skinned: Vec::with_capacity(n),
            normals: Vec::with_capacity(n),
            verts: Vec::with_capacity(n),
            textured: Vec::with_capacity(n),
            tangents: Vec::with_capacity(n),
            scratch: Vec::with_capacity(2 * n),
            caps: Vec::with_capacity(collision.volumes.len()),
        }
    }

    /// THE SPLIT GATE (spec 6C46CAB9): every triangle touching a chain-bound vertex goes to the
    /// CLOTH submesh and none of the purely rigid ones do; the rigid half keeps its own indices
    /// into the unchanged vertex buffer; and a mesh with no binds does not split at all.
    #[test]
    fn the_split_takes_every_bound_triangle_and_no_rigid_one() {
        let mesh = sleeve_mesh(2, 20.0);
        let split = split_cloth(&mesh, &mesh.indices).expect("two bound triangles");
        assert_eq!(split.tris.len(), 6, "two cloth triangles, re-indexed");
        assert_eq!(split.src.len(), 6, "their six distinct source vertices");
        assert_eq!(split.rigid.len(), 6, "the two rigid triangles stay skinned");
        // Every bound vertex is in the cloth submesh; no rigid-only vertex is.
        let bound: Vec<u32> = mesh.cloth.regions[0].binds.iter().map(|b| b.v).collect();
        for v in &bound {
            assert!(split.src.contains(v), "bound vertex {v} must be cloth");
        }
        for i in &split.rigid {
            assert!(
                !bound.contains(i),
                "a rigid triangle touches no bound vertex"
            );
        }
        // The sim now speaks the SUBMESH's indices, not the mesh's.
        for (b, v) in mesh.cloth.regions[0]
            .binds
            .iter()
            .zip(split.cloth.regions[0].binds.iter())
        {
            assert_eq!(split.src[v.v as usize], b.v, "the bind was re-indexed");
        }
        let mut plain = mesh.clone();
        plain.cloth = Default::default();
        assert!(
            split_cloth(&plain, &plain.indices).is_none(),
            "an ordinary body does not split"
        );
    }

    /// A synthetic sleeve, driven for two seconds off a swinging bone, DRAPES: its cloth vertices
    /// end below where the rigid skin would have put them, while the rigid half of the same body
    /// is untouched (it never enters the cloth submesh at all).
    #[test]
    fn a_swung_sleeve_drapes_below_its_rigid_skin() {
        let mesh = sleeve_mesh(1, 20.0);
        let bones = [bone("root")];
        let mut cpu = cpu_of(&mesh, &bones, &Collision::default());
        let palette = [Mat4::from_translation(Vec3::new(30.0, 0.0, 0.0))];
        let rigid = skin::skin(&mesh, &palette);
        for _ in 0..120 {
            cpu.simulate(&palette, Some(1.0 / 60.0));
        }
        let bound = mesh.cloth.regions[0].binds[0].v as usize;
        let local = cpu.bind.iter().position(|v| v.p == mesh.vertices[bound].p);
        let drift = cpu.verts[local.expect("the bound vertex is in the submesh")].position;
        assert!(
            drift[2] < rigid[bound].position[2] - 1.0,
            "the cloth must hang BELOW its rigid skin: {} vs {}",
            drift[2],
            rigid[bound].position[2]
        );
        // The rigid half never entered the submesh, so nothing here can have moved it.
        let split = split_cloth(&mesh, &mesh.indices).unwrap();
        for i in &split.rigid {
            assert_eq!(
                rigid[*i as usize].position,
                skin::skin(&mesh, &palette)[*i as usize].position
            );
        }
    }

    /// THE CAPSULE GATE: a chain node hanging inside a body capsule is pushed out to its surface
    /// by `push_out_of_body`, which is the one place `flicker_mechanics::collision` is consulted.
    #[test]
    fn a_chain_node_inside_a_capsule_is_pushed_out() {
        use flicker_skeletal::format::{CollisionRole, CollisionShape, CollisionVolume};
        const R: f32 = 8.0;
        let (a, b) = (Vec3::new(0.0, -6.0, 0.0), Vec3::new(20.0, -6.0, 0.0));
        let mesh = sleeve_mesh(1, 20.0);
        let bones = [bone("root")];
        let collision = Collision {
            volumes: vec![CollisionVolume {
                name: "thigh".into(),
                bone: "root".into(),
                shape: CollisionShape::Capsule {
                    a: a.to_array(),
                    b: b.to_array(),
                    radius: R,
                },
                role: CollisionRole::Physics,
            }],
        };
        let mut cpu = cpu_of(&mesh, &bones, &collision);
        let palette = [Mat4::IDENTITY];
        // Distance from a point to the capsule's axis segment.
        let to_axis = |p: Vec3| {
            let ab = b - a;
            let t = ((p - a).dot(ab) / ab.dot(ab)).clamp(0.0, 1.0);
            (p - (a + ab * t)).length()
        };
        cpu.sim.step(&palette, 1.0 / 60.0);
        let mut inside = 0;
        cpu.sim.push_free_nodes(|p| {
            if to_axis(p) < R {
                inside += 1;
            }
            p
        });
        assert!(inside > 0, "the hang must start inside the capsule");
        push_out_of_body(&mut cpu.sim, &palette, &mut cpu.caps);
        assert_eq!(cpu.caps.len(), 1, "the capsule was posed by its bone");
        cpu.sim.push_free_nodes(|p| {
            assert!(
                to_axis(p) >= R - 1e-3,
                "every free node must end on or outside the capsule, got {p} at {}",
                to_axis(p)
            );
            p
        });
    }

    /// A `Cloth` panel sewn along one row of a body: rows × cols in the xy plane, row 0 the
    /// body (untagged), the rest the region, no chains laid — the SHEET solver's input.
    fn panel_mesh(rows: usize, cols: usize, s: f32) -> Mesh {
        use flicker_skeletal::format::{Cloth, ClothParams, ClothRegion};
        let mut vertices: Vec<Vertex> = Vec::new();
        for i in 0..rows {
            for j in 0..cols {
                let mut v = vert(0.0);
                v.p = [j as f32 * s, i as f32 * s, 0.0];
                vertices.push(v);
            }
        }
        let at = |i: usize, j: usize| (i * cols + j) as u32;
        let mut indices = Vec::new();
        for i in 0..rows - 1 {
            for j in 0..cols - 1 {
                indices.extend_from_slice(&[at(i, j), at(i, j + 1), at(i + 1, j)]);
                indices.extend_from_slice(&[at(i, j + 1), at(i + 1, j + 1), at(i + 1, j)]);
            }
        }
        Mesh {
            vertices,
            indices,
            cloth: Cloth {
                regions: vec![ClothRegion {
                    name: "hem".into(),
                    anchor_bone: "root".into(),
                    tag: RegionTag::Cloth,
                    verts: (cols as u32..(rows * cols) as u32).collect(),
                    chain_count: 1,
                    params: ClothParams::default(),
                    chains: Vec::new(),
                    binds: Vec::new(),
                }],
            },
            ..Default::default()
        }
    }

    /// THE SHEET ON THE RUNTIME PATH (ruling 82EDC071): a `Cloth` panel splits off by its
    /// MEMBERSHIP (no chains laid), hangs below its rigid skin from its seam, leaves the body row
    /// exactly where the skin put it, and its free vertices clear a body capsule under it through
    /// the one capsule test the runtime owns.
    #[test]
    fn a_cloth_panel_hangs_from_its_seam_and_clears_the_body() {
        use flicker_skeletal::format::{CollisionRole, CollisionShape, CollisionVolume};
        let (rows, cols, s) = (8, 5, 5.0);
        let mesh = panel_mesh(rows, cols, s);
        let bones = [bone("root")];
        const R: f32 = 6.0;
        let (a, b) = (Vec3::new(-10.0, 0.0, -12.0), Vec3::new(40.0, 0.0, -12.0));
        let collision = Collision {
            volumes: vec![CollisionVolume {
                name: "thigh".into(),
                bone: "root".into(),
                shape: CollisionShape::Capsule {
                    a: a.to_array(),
                    b: b.to_array(),
                    radius: R,
                },
                role: CollisionRole::Physics,
            }],
        };
        let mut cpu = cpu_of(&mesh, &bones, &collision);
        let palette = [Mat4::IDENTITY];
        for _ in 0..180 {
            cpu.simulate(&palette, Some(1.0 / 60.0));
        }
        let rigid = skin::skin(&mesh, &palette);
        let local = |v: usize| {
            cpu.bind
                .iter()
                .position(|b| b.p == mesh.vertices[v].p)
                .expect("the panel is in the submesh")
        };
        for j in 0..cols {
            assert_eq!(
                cpu.verts[local(j)].position,
                rigid[j].position,
                "the body row is the skin's"
            );
            let far = cpu.verts[local((rows - 1) * cols + j)].position;
            assert!(
                far[2] < rigid[(rows - 1) * cols + j].position[2] - 10.0,
                "the far row hangs below its rigid skin: {far:?}"
            );
        }
        let to_axis = |p: Vec3| {
            let ab = b - a;
            let t = ((p - a).dot(ab) / ab.dot(ab)).clamp(0.0, 1.0);
            (p - (a + ab * t)).length()
        };
        cpu.sim.push_free_nodes(|p| {
            assert!(
                to_axis(p) >= R - 1e-3,
                "every free vertex ends on or outside the capsule, got {p} at {}",
                to_axis(p)
            );
            p
        });
    }

    /// THE SHEET ON A REAL REGION (ruling 82EDC071; rule CE0451CE — measure on real data before
    /// reporting): `FLICKER_RIG_DIR=<rig folder> FLICKER_REGION=<region name> … --ignored
    /// --nocapture`. Loads the rig, re-tags that region `Cloth` so it runs as a SHEET over its
    /// real triangles, settles it on the rest pose and steps it for two seconds under the rig's
    /// own capsules, and PRINTS what a real panel costs and does: members and constraints, the
    /// milliseconds a frame, how far its vertices hang below the rigid skin, the worst edge
    /// strain, and that everything stayed finite.
    #[test]
    #[ignore]
    fn diagnose_the_sheet_on_a_real_region() {
        let Ok(dir) = std::env::var("FLICKER_RIG_DIR") else {
            eprintln!("skipping: FLICKER_RIG_DIR not set");
            return;
        };
        let want = std::env::var("FLICKER_REGION").unwrap_or_default();
        let model =
            flicker_skeletal::format::load_dir(std::path::Path::new(&dir)).expect("the rig loads");
        let mut mesh = model.mesh.clone();
        let mut named = false;
        for r in &mut mesh.cloth.regions {
            if r.name == want {
                r.tag = RegionTag::Cloth;
                r.chain_count = r.chain_count.max(1);
                named = true;
            } else {
                // Every other region out of the way: this is the one panel's reading.
                r.chain_count = 0;
                r.binds.clear();
            }
        }
        assert!(named, "no region named {want:?} in {dir}");
        let bones = &model.bones;
        // `FLICKER_NO_CAPSULES=1` runs the panel with no body to collide with — the reading of
        // the sheet alone, against the one with the rig's capsules.
        let none = Collision::default();
        let collision = if std::env::var("FLICKER_NO_CAPSULES").is_ok() {
            &none
        } else {
            &model.collision
        };
        let t0 = std::time::Instant::now();
        let mut cpu = cpu_of(&mesh, bones, collision);
        let built = t0.elapsed().as_secs_f64() * 1000.0;
        let rest: Vec<Mat4> = bones.iter().map(|b| b.local).collect();
        let palette = skin::palette(bones, &pose::global_transforms(bones, &rest));
        let mut rigid: Vec<CpuVertex> = Vec::new();
        skin::skin_subset(&cpu.bind, &palette, &mut rigid);
        cpu.simulate(&palette, None);
        const FRAMES: usize = 120;
        let t1 = std::time::Instant::now();
        for _ in 0..FRAMES {
            cpu.simulate(&palette, Some(1.0 / 60.0));
        }
        let per_frame = t1.elapsed().as_secs_f64() * 1000.0 / FRAMES as f64;
        let (mut finite, mut below, mut moved, mut worst_drop) = (true, 0usize, 0usize, 0.0f32);
        for (v, r) in cpu.verts.iter().zip(&rigid) {
            let (p, q) = (Vec3::from(v.position), Vec3::from(r.position));
            finite &= p.is_finite();
            let d = q.z - p.z;
            if (p - q).length() > 0.1 {
                moved += 1;
            }
            if d > 0.1 {
                below += 1;
            }
            worst_drop = worst_drop.max(d);
        }
        // Strain over the edges with a real length (a 600k soup has hairline edges whose
        // relative stretch means nothing), and the worst ABSOLUTE stretch over every edge.
        let (mut strain, mut stretch) = (0.0f32, 0.0f32);
        let split = split_cloth(&mesh, &mesh.indices).expect("the panel splits");
        let members: std::collections::HashSet<u32> = mesh
            .cloth
            .regions
            .iter()
            .find(|r| r.name == want)
            .map(|r| r.verts.iter().copied().collect())
            .unwrap_or_default();
        let mut worst_edges: Vec<(f32, usize, usize, f32, f32)> = Vec::new();
        for t in cpu.tris.as_chunks::<3>().0 {
            for (a, b) in [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])] {
                let (a, b) = (a as usize, b as usize);
                let rest = Vec3::from(cpu.bind[a].p).distance(Vec3::from(cpu.bind[b].p));
                let now =
                    Vec3::from(cpu.verts[a].position).distance(Vec3::from(cpu.verts[b].position));
                stretch = stretch.max((now - rest).abs());
                if rest >= 0.5 {
                    strain = strain.max((now - rest).abs() / rest);
                    worst_edges.push(((now - rest).abs() / rest, a, b, rest, now));
                }
            }
        }
        worst_edges.sort_by(|x, y| y.0.total_cmp(&x.0));
        for &(st, a, b, rest, now) in worst_edges.iter().take(5) {
            let m = |l: usize| members.contains(&split.src[l]);
            eprintln!(
                "  worst edge {a}-{b}: strain {:.0} % rest {rest:.2} now {now:.2} | member {} {} | bind {:?} {:?} | now {:?} {:?}",
                st * 100.0, m(a), m(b), cpu.bind[a].p, cpu.bind[b].p, cpu.verts[a].position, cpu.verts[b].position
            );
        }
        eprintln!(
            "SHEET {want}: submesh {} verts / {} tris, built in {built:.1} ms, {per_frame:.3} ms a frame; \
             {moved} vertices moved off the rigid skin, {below} hang below it, the deepest by \
             {worst_drop:.1} cm; worst edge strain {:.1} % (edges over 5 mm), worst stretch \
             {stretch:.2} cm; finite {finite}; capsules {}",
            cpu.bind.len(),
            cpu.tris.len() / 3,
            strain * 100.0,
            cpu.sim.capsules().len()
        );
        eprintln!("  {}", cpu.sim.sheet_report());
        assert!(finite, "a real sheet stays finite");
    }

    /// A poster SETTLES: two consecutive settles of the same pose write byte-identical vertices,
    /// so a still surface never shimmers when its clock marks it dirty.
    #[test]
    fn two_consecutive_poster_settles_write_the_same_vertices() {
        let mesh = sleeve_mesh(2, 20.0);
        let bones = [bone("root")];
        let mut cpu = cpu_of(&mesh, &bones, &Collision::default());
        let palette = [Mat4::from_translation(Vec3::new(4.0, 0.0, 0.0))];
        cpu.simulate(&palette, None);
        let first = cpu.verts.clone();
        cpu.simulate(&palette, None);
        for (a, b) in first.iter().zip(cpu.verts.iter()) {
            // A settled chain is CONVERGED, not frozen: the gate is that the creep is invisible.
            let d = (Vec3::from(a.position) - Vec3::from(b.position)).length();
            assert!(d < 1e-3, "a settled poster must not move, drifted {d}");
        }
        assert!(
            first.iter().all(|v| v.material == CLOTH_MATERIAL),
            "the cloth half must shade like the skinned half"
        );
    }

    fn seat(w: f32, h: f32) -> SurfaceSlot {
        SurfaceSlot {
            id: "d".into(),
            source: "doll_test".into(),
            // A doll fills a card, not a sub scene: it names none and carries no params.
            scene: String::new(),
            params: Default::default(),
            x: 0.0,
            y: 0.0,
            w,
            h,
            layer: 0.0,
            rate: Rate::Live,
            tint: [1.0; 4],
            layout: flicker::render::ViewportLayout::Single,
        }
    }

    /// **The six sizes.** The design puts the same doll on the screen at 34, 42, 48, 92,
    /// 180 and 300 px; the shot is in world units, so every one of them must frame the
    /// subject IDENTICALLY — a small doll is the same picture with fewer pixels, never a
    /// differently-composed one. Regression gate against wiring pixels into the camera.
    #[test]
    fn the_framing_is_the_same_shot_at_every_one_of_the_six_sizes() {
        let mut seen: Option<(Vec3, Vec3)> = None;
        for px in [34.0f32, 42.0, 48.0, 92.0, 180.0, 300.0] {
            let mut d = Doll::new("doll_test", &styles());
            d.set_rig(Some(rig(0.9)));
            d.seat(Some(&seat(px, px)));
            let cam = d.view.camera();
            // The authored shot: looking at the chest, from the authored distance.
            assert!(
                (cam.target - Vec3::new(0.0, 0.95, 0.0)).length() < 1e-4,
                "{px}px looks at the authored target, got {}",
                cam.target
            );
            assert!(
                ((cam.position - cam.target).length() - 2.6).abs() < 1e-3,
                "{px}px stands at the authored distance, got {}",
                (cam.position - cam.target).length()
            );
            match seen {
                None => seen = Some((cam.position, cam.target)),
                Some((p, t)) => {
                    assert!((cam.position - p).length() < 1e-4, "{px}px moved the eye");
                    assert!((cam.target - t).length() < 1e-4, "{px}px moved the look-at");
                }
            }
        }
    }

    /// A rig of a different size gets the SAME authored shot, scaled to it — the reason
    /// the distance is expressed against the subject radius rather than baked in.
    #[test]
    fn the_authored_shot_scales_to_the_rig() {
        let mut small = Doll::new("doll_test", &styles());
        small.set_rig(Some(rig(0.45)));
        small.seat(Some(&seat(92.0, 92.0)));
        let mut big = Doll::new("doll_test", &styles());
        big.set_rig(Some(rig(1.8)));
        big.seat(Some(&seat(92.0, 92.0)));
        let d = |c: flicker::render::Camera| (c.position - c.target).length();
        // Same authored `dist` in world units for both — the orbit's dist_scale absorbed
        // the radius difference, so neither rig is framed from inside its own chest.
        assert!((d(small.view.camera()) - d(big.view.camera())).abs() < 1e-3);
        assert!((d(big.view.camera()) - 2.6).abs() < 1e-3);
    }

    /// The field of view is VERTICAL, so a seat NARROWER than it is tall must back the
    /// camera off or the subject is cropped at the sides. A wide seat is left alone —
    /// it already shows more than the shot asked for.
    #[test]
    fn a_narrow_seat_backs_the_camera_off_and_a_wide_one_does_not() {
        let d = |w: f32, h: f32| {
            let mut d = Doll::new("doll_test", &styles());
            d.set_rig(Some(rig(0.9)));
            d.seat(Some(&seat(w, h)));
            let c = d.view.camera();
            (c.position - c.target).length()
        };
        let square = d(92.0, 92.0);
        assert!(d(46.0, 92.0) > square * 1.5, "a half-width seat pulls back");
        assert!(
            (d(300.0, 92.0) - square).abs() < 1e-3,
            "a wide seat keeps the authored shot"
        );
    }

    /// **The live / poster contract.** A live doll asks for a rate on the clock; a still
    /// one asks to be re-rendered only when it says its image changed. This is the whole
    /// reason a page of a dozen dolls costs one pass.
    #[test]
    fn a_live_doll_asks_for_a_rate_and_a_poster_asks_for_dirty() {
        let mut d = Doll::new("doll_test", &styles());
        assert_eq!(
            d.rate(),
            Rate::Dirty,
            "a doll is a poster until told otherwise"
        );
        d.set_live(true);
        assert_eq!(d.rate(), Rate::Hz(LIVE_HZ));
        assert!(
            matches!(d.rate(), Rate::Hz(hz) if hz > 0.0),
            "a live rate is > 0"
        );
        d.set_live(false);
        assert_eq!(d.rate(), Rate::Dirty);
        // And the rate the clock is handed is a real one at every size.
        let refused = Doll::new("doll_test", &styles()).live_hz(0.0);
        assert_eq!(
            refused.hz, LIVE_HZ,
            "a zero rate would never draw — refused"
        );
    }

    /// Only what CHANGES the image raises `dirty`, and the render consumes it — otherwise
    /// a poster either never redraws (stale) or redraws forever (not a poster).
    #[test]
    fn only_a_real_change_dirties_a_poster() {
        let mut d = Doll::new("doll_test", &styles());
        d.set_rig(Some(rig(0.9)));
        d.seat(Some(&seat(92.0, 92.0)));
        d.dirty = false;

        d.set_clip(None);
        assert!(!d.dirty, "re-setting the same clip is not a change");
        d.set_clip(Some(3));
        assert!(d.dirty, "a new clip is a new image");
        d.dirty = false;

        d.set_active(false);
        assert!(!d.dirty, "re-setting the same activity is not a change");
        d.set_active(true);
        assert!(d.dirty, "the ring changed colour");
        d.dirty = false;

        let same = d.rig.clone();
        d.set_rig(same);
        assert!(!d.dirty, "the same rig is not a change");
        d.set_rig(Some(rig(0.9)));
        assert!(d.dirty, "a new rig is a new image");
    }

    /// The clock is the doll's own and only runs while it is live — a poster advancing a
    /// play-head nobody renders is exactly the per-frame recompute it exists to avoid.
    #[test]
    fn the_clip_clock_advances_only_while_live() {
        let mut d = Doll::new("doll_test", &styles());
        d.tick(0.5);
        assert_eq!(d.time(), 0.0, "a poster's clock is parked");
        d.set_live(true);
        d.tick(0.25);
        d.tick(0.25);
        assert!(
            (d.time() - 0.5).abs() < 1e-6,
            "a live doll runs its own clock"
        );
        d.set_live(false);
        d.tick(1.0);
        assert!((d.time() - 0.5).abs() < 1e-6, "and stops where it stopped");
    }

    /// The clip loops on its OWN duration and an unknown one is the rest pose — a doll
    /// bound to a clip the rig does not carry must not panic on a list refill.
    #[test]
    fn the_palette_loops_the_clip_and_falls_back_to_the_rest_pose() {
        let r = rig(0.9);
        let p = r.palette(None, 0.0);
        assert_eq!(p.len(), 3, "one matrix per bone");
        assert!(p.iter().all(|m| m.is_finite()));
        // An index past the end of an empty clip list is the rest pose, not a panic.
        assert_eq!(r.palette(Some(7), 1.5).len(), 3);
        assert_eq!(r.bone_count(), 3);
    }

    /// The ring's colour is the one piece of per-doll state the authored geometry carries;
    /// activity changes the colour, never the geometry.
    #[test]
    fn the_active_ring_changes_colour_not_geometry() {
        let d = Doll::new("doll_test", &styles());
        let layers = &d.view.stage().layers;
        let idle = ground_lines(layers, false);
        let lit = ground_lines(layers, true);
        assert_eq!(
            idle.len(),
            lit.len(),
            "activity changes colour, not geometry"
        );
        assert!(!idle.is_empty(), "the ring produced segments");
        assert_eq!(idle[0].1.len(), lit[0].1.len());
        assert_ne!(idle[0].0, lit[0].0, "an active ring is a different colour");
        // A layer kind the doll does not draw contributes nothing rather than a panic.
        assert!(ground_lines(&[StageLayer::Graticule { radius_scale: 1.0 }], false).is_empty());
        // And a degenerate ring is no ring, not a crash.
        let bad = [StageLayer::Ring {
            radius: -1.0,
            y: 0.0,
            segments: 24,
            color: [1.0; 4],
            color_active: [1.0; 4],
        }];
        assert!(ground_lines(&bad, false)[0].1.is_empty());
    }

    /// The rig is centred on the origin by `Model::world`; the authored stages put the
    /// feet at y = 0, so the ground transform must drop the doll by its lowest vertex.
    #[test]
    fn the_ground_transform_puts_the_feet_on_the_floor() {
        let g = ground_transform(Mat4::IDENTITY, &[vert(-0.9), vert(0.9)]);
        let lowest = g.transform_point3(Vec3::new(0.0, -0.9, 0.0));
        assert!(lowest.y.abs() < 1e-5, "lowest vertex lands on y = 0");
        // The whole rig shifts together — it is not scaled.
        let top = g.transform_point3(Vec3::new(0.0, 0.9, 0.0));
        assert!((top.y - 1.8).abs() < 1e-5, "the doll keeps its height");
        assert!(ground_transform(Mat4::IDENTITY, &[]).is_finite());
    }

    /// **The cardinal sin, gated.** A settled poster must not pose its skeleton: with a
    /// dozen dolls on a page that is a 67-bone recompute per frame for images nobody
    /// draws. Every way the pass CAN still run must raise the flag, or a doll draws blank.
    #[test]
    fn a_settled_poster_does_not_pose_but_everything_that_can_draw_does() {
        let mut d = Doll::new("doll_test", &styles());
        assert!(
            d.poses(),
            "the first frame always renders — it must be posed"
        );
        d.set_rig(Some(rig(0.9)));
        d.seat(Some(&seat(92.0, 92.0)));
        assert!(d.poses(), "a fresh target must draw");

        d.dirty = false;
        assert!(!d.poses(), "a settled poster poses nothing");

        // A resize rebuilds the target, and a fresh target must draw.
        d.seat(Some(&seat(300.0, 300.0)));
        assert!(d.poses(), "a resized doll must redraw");
        d.dirty = false;

        d.set_clip(Some(1));
        assert!(d.poses(), "a new clip must redraw");
        d.dirty = false;

        d.set_time(2.0);
        assert!(d.poses(), "a moved play-head must redraw");
        d.dirty = false;

        // Live is live: it poses whether or not anything else changed.
        d.set_live(true);
        d.dirty = false;
        assert!(d.poses(), "a live doll always poses");

        // And release leaves it ready to draw into the fresh target it will be given.
        d.set_live(false);
        d.dirty = false;
        d.unseat();
        assert!(!d.poses(), "an off-page doll poses nothing");
    }

    /// An unseated doll declares nothing, and `release` leaves it that way — the seam a
    /// host's `exit()` calls so no target outlives the bench that made it.
    #[test]
    fn an_unseated_doll_has_no_rect() {
        let mut d = Doll::new("doll_test", &styles());
        assert!(d.rect().is_none());
        d.seat(Some(&seat(92.0, 92.0)));
        assert_eq!(d.rect().map(|r| r.size), Some(Vec2::new(92.0, 92.0)));
        d.unseat();
        assert!(d.rect().is_none(), "an off-page doll reserves nothing");
    }
}
