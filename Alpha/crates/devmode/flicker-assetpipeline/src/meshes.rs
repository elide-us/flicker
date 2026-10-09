//! **The bench's GPU-side caches** — what the rig panels draw, as handles this bench
//! owns: the source mesh (textured when the folder ships maps), its wireframe twin, the
//! CPU-skinned pose of a rigged source, the fitting body a prop is mounted against, and
//! the bake preview's skinned mesh. Each cache is keyed by the document's generations and
//! re-uploads only when its key moves; the panels receive [`Draw`] items and never touch
//! the renderer's allocation themselves.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use flicker::render::{
    build_textured_verts, Mat4, MeshDrawOptions, MeshHandle, MeshIndices, MeshVertex, PbrMaps,
    Renderer, TextureHandle, TexturedMeshHandle, Vec3,
};
use flicker_content::{attach_world, fitting_base, source_maps, Fit, SourceMaps};
use flicker_mechanics::gait::{
    kind_of, speed_for, FlatFloor, GaitKind, Locomotion, LocomotionFamily,
};
use flicker_rigview::{Draw, SkinnedBody};
use flicker_skeletal::format::{Bone as SkelBone, Pattern, ResolvedClip};
use flicker_skeletal::pose::{global_transforms, sample_local_poses};
use flicker_skeletal::skin;
use flicker_skeletal::state::{ClipSource, GeneratedGait};

use crate::services::{skin_source_verts, Document, PropFit, SOCKETS};

/// The fitting body (GolemBase) is a dense mesh; above this many vertices the reference
/// view shows its skeleton only — a fit needs the joints, not a 50 MB body.
pub(crate) const BASE_MESH_BUDGET: usize = 450_000;
// (450 000 = the ultra bodies' 150 000 triangles as the unwelded corners a baked rig carries —
// GolemBaseV2 is the fitting body since 2026-09-07; the 256 MiB buffer cap went the same day.)

/// A cache key: which candidate file of which folder the upload came from.
type PreviewKey = (PathBuf, usize);

/// An uploaded mesh — textured when the source shipped a base-colour map, flat otherwise.
#[derive(Clone, Copy)]
pub(crate) enum Uploaded {
    Textured {
        mesh: TexturedMeshHandle,
        albedo: TextureHandle,
        maps: PbrMaps,
    },
    Flat(MeshHandle),
}

impl Uploaded {
    /// The draw item for this mesh at `world`; a flat mesh takes `flat_tint`.
    pub(crate) fn draw(self, world: Mat4, flat_tint: [f32; 4]) -> Draw {
        match self {
            Uploaded::Textured { mesh, albedo, maps } => Draw::Textured {
                mesh,
                albedo,
                maps,
                world,
            },
            Uploaded::Flat(mesh) => Draw::Mesh {
                mesh,
                world,
                options: MeshDrawOptions {
                    tint: flat_tint,
                    ..Default::default()
                },
            },
        }
    }

    fn free(self, r: &mut Renderer) {
        match self {
            Uploaded::Textured { mesh, .. } => r.free_textured_mesh(mesh),
            Uploaded::Flat(h) => r.free_mesh(h),
        }
    }
}

/// Load one PNG map through the renderer (sRGB for colour, linear for data), cached by path.
fn load_map(
    r: &mut Renderer,
    cache: &mut HashMap<PathBuf, TextureHandle>,
    path: &Path,
    srgb: bool,
) -> Option<TextureHandle> {
    if let Some(h) = cache.get(path) {
        return Some(*h);
    }
    match image::open(path) {
        Ok(img) => {
            let rgba = img.to_rgba8();
            let (w, h) = rgba.dimensions();
            let handle = if srgb {
                r.load_texture(rgba.as_raw(), w, h)
            } else {
                r.load_texture_linear(rgba.as_raw(), w, h)
            };
            cache.insert(path.to_path_buf(), handle);
            tracing::info!(map = %path.display(), w, h, srgb, "clayworks: texture loaded");
            Some(handle)
        }
        Err(e) => {
            tracing::warn!(map = %path.display(), "clayworks: texture failed ({e}); using the default");
            None
        }
    }
}

/// Upload a mesh with its source maps: the PBR path when a base-colour map resolves and
/// the UVs line up, the flat path otherwise.
fn upload_preview(
    r: &mut Renderer,
    cache: &mut HashMap<PathBuf, TextureHandle>,
    maps: &SourceMaps,
    verts: &[MeshVertex],
    uvs: &[[f32; 2]],
    indices: &[u32],
) -> Uploaded {
    // The converter emits no index list when the vertices are already sequential.
    let seq: Vec<u32>;
    let idx: &[u32] = if indices.is_empty() {
        seq = (0..verts.len() as u32).collect();
        &seq
    } else {
        indices
    };

    let albedo = maps
        .base_color
        .as_deref()
        .filter(|_| uvs.len() == verts.len())
        .and_then(|p| load_map(r, cache, p, true));
    let Some(albedo) = albedo else {
        return Uploaded::Flat(r.upload_mesh(verts, MeshIndices::U32(idx)));
    };

    let flat: Vec<usize> = idx
        .iter()
        .map(|&i| i as usize)
        .filter(|&i| i < verts.len())
        .collect();
    let tv = build_textured_verts(
        0..flat.len(),
        |k| verts[flat[k]].position,
        |k| verts[flat[k]].normal,
        |k| uvs[flat[k]],
    );
    let li: Vec<u32> = (0..tv.len() as u32).collect();
    let mesh = r.upload_textured_mesh(&tv, MeshIndices::U32(&li));
    let normal = maps
        .normal
        .as_deref()
        .and_then(|p| load_map(r, cache, p, false));
    let roughness = maps
        .roughness
        .as_deref()
        .and_then(|p| load_map(r, cache, p, false));
    let metalness = maps
        .metalness
        .as_deref()
        .and_then(|p| load_map(r, cache, p, false));
    Uploaded::Textured {
        mesh,
        albedo,
        maps: PbrMaps {
            normal,
            roughness,
            metalness,
            ao: None,
            emit: None,
        },
    }
}

/// The reference BODY a prop is fitted against: the fitting base rig's skeleton (always)
/// and its mesh (when within budget), with the maps its material names.
pub(crate) struct BasePreview {
    names: Vec<String>,
    pub(crate) parents: Vec<i32>,
    pub(crate) globals: Vec<Mat4>,
    ibind: Vec<[f32; 16]>,
    pub(crate) centre: Vec3,
    pub(crate) radius: f32,
    /// The feet plane relative to `centre` (subtract nothing: add `centre.z` for world).
    pub(crate) floor: f32,
    pub(crate) verts: Vec<MeshVertex>,
    uvs: Vec<[f32; 2]>,
    pub(crate) indices: Vec<u32>,
    maps: SourceMaps,
}

impl BasePreview {
    /// Skeleton-only deserialize of the fitting body; `None` when it is absent or empty.
    pub(crate) fn load() -> Option<Self> {
        #[derive(serde::Deserialize)]
        struct BaseRig {
            #[serde(default)]
            skeleton: flicker_skeletal::format::Skeleton,
            #[serde(default)]
            mesh: flicker_skeletal::format::Mesh,
        }
        let base_path = fitting_base();
        let text = flicker_content::package::read_text(&base_path).ok()?;
        let rig: BaseRig = serde_json::from_str(&text).ok()?;
        if rig.skeleton.bones.is_empty() {
            return None;
        }
        let names: Vec<String> = rig.skeleton.bones.iter().map(|b| b.name.clone()).collect();
        let parents: Vec<i32> = rig.skeleton.bones.iter().map(|b| b.parent).collect();
        let ibind: Vec<[f32; 16]> = rig.skeleton.bones.iter().map(|b| b.inverse_bind).collect();
        let globals: Vec<Mat4> = ibind
            .iter()
            .map(|m| Mat4::from_cols_array(m).inverse())
            .collect();

        let too_dense = rig.mesh.vertices.len() > BASE_MESH_BUDGET;
        if too_dense {
            tracing::warn!(
                verts = rig.mesh.vertices.len(),
                budget = BASE_MESH_BUDGET,
                "clayworks: fitting body over budget — showing its skeleton only"
            );
        }
        let (verts, uvs, indices): (Vec<MeshVertex>, Vec<[f32; 2]>, Vec<u32>) = if too_dense {
            (Vec::new(), Vec::new(), Vec::new())
        } else {
            let v: Vec<MeshVertex> = rig
                .mesh
                .vertices
                .iter()
                .map(|x| MeshVertex {
                    position: x.p,
                    normal: x.n,
                    material: 0,
                })
                .collect();
            let uv: Vec<[f32; 2]> = rig.mesh.vertices.iter().map(|x| x.uv).collect();
            let i: Vec<u32> = if rig.mesh.indices.is_empty() {
                (0..v.len() as u32).collect()
            } else {
                rig.mesh.indices.clone()
            };
            (v, uv, i)
        };

        let dir = base_path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."));
        let mat = rig.mesh.materials.first();
        let named = |s: &str| (!s.is_empty()).then(|| dir.join(s));
        let maps = SourceMaps {
            base_color: mat.and_then(|m| named(&m.base_color)),
            metalness: mat.and_then(|m| named(&m.metalness)),
            roughness: mat.and_then(|m| named(&m.roughness)),
            normal: mat.and_then(|m| named(&m.normal)),
        };

        let (mut lo, mut hi) = (Vec3::splat(f32::INFINITY), Vec3::splat(f32::NEG_INFINITY));
        if verts.is_empty() {
            for g in &globals {
                let p = g.w_axis.truncate();
                lo = lo.min(p);
                hi = hi.max(p);
            }
        } else {
            for v in &verts {
                let p = Vec3::from(v.position);
                lo = lo.min(p);
                hi = hi.max(p);
            }
        }
        let centre = (lo + hi) * 0.5;
        let radius = ((hi - lo).max_element() * 0.5).max(50.0);
        let floor = lo.z - centre.z;
        Some(Self {
            names,
            parents,
            globals,
            ibind,
            centre,
            radius,
            floor,
            verts,
            uvs,
            indices,
            maps,
        })
    }

    fn socket_index(&self, name: &str) -> Option<usize> {
        self.names.iter().position(|n| n == name)
    }

    /// Where a piece mounted by `fit` sits in the body's space (identity when the fit's
    /// socket is not a bone of this body).
    pub(crate) fn socket_world(&self, fit: &PropFit) -> Mat4 {
        let socket_name = SOCKETS
            .get(fit.socket)
            .map(|(id, _)| *id)
            .unwrap_or("pelvis");
        self.socket_index(socket_name)
            .map(|i| {
                let f = Fit {
                    socket: socket_name.to_string(),
                    offset: fit.offset,
                    rot_deg: fit.rot,
                    scale: fit.scale,
                    uniform: fit.uniform,
                };
                attach_world(&self.ibind[i], &f.to_attach())
            })
            .unwrap_or(Mat4::IDENTITY)
    }
}

/// A GENERATED WALK for a creature pattern's preview (G6 of the gait/IK design): the
/// `flicker-mechanics` driver on a TREADMILL — the body walks its pack's ground gait at the
/// speed that means it, on a floor at the rig's feet, while the view holds it over the
/// origin, so the feet plant, swing and carry the trunk exactly as the runtime will move it.
pub(crate) struct GeneratedWalk {
    driver: Locomotion,
    /// The pack's ground gait the preview shows (its walk when it has one; a stand when the
    /// pack has only flight states, as the bat's).
    pub(crate) kind: GaitKind,
    rest: Vec<Mat4>,
    floor: FlatFloor,
    last_tick: f32,
}

impl GeneratedWalk {
    /// The walk for `pattern`'s bake, read off the pattern's DEFAULT PACK (the pack contract's
    /// `Generated` sources, G5): `None` when the pack has no generated state (the pattern
    /// animates from clips), when there is no pack, or when the limb model finds no planted
    /// limbs on the rig.
    pub(crate) fn new(bones: &[SkelBone], pattern: Pattern) -> Option<Self> {
        let (kind, family) = pack_gait(pattern)?;
        let names: Vec<&str> = bones.iter().map(|b| b.name.as_str()).collect();
        let parents: Vec<i32> = bones.iter().map(|b| b.parent).collect();
        let locals: Vec<Mat4> = bones.iter().map(|b| b.local).collect();
        let rest = global_transforms(bones, &locals);
        let driver = Locomotion::new(&names, &parents, &rest, family);
        if driver.feet.is_empty() {
            return None;
        }
        let floor = FlatFloor {
            height: driver
                .feet
                .iter()
                .map(|f| rest[f.limb.effector].w_axis.z)
                .fold(f32::INFINITY, f32::min),
        };
        Some(Self {
            driver,
            kind,
            rest,
            floor,
            last_tick: 0.0,
        })
    }

    /// The speed (cm/s) that means the pack's gait on this body's legs.
    pub(crate) fn speed(&self) -> f32 {
        speed_for(self.kind, self.driver.leg_len)
    }

    /// Step the walk to `tick` of a clock at `hz` that wraps at `duration` ticks, and hold
    /// the body over the origin. Returns the bones' world frames.
    pub(crate) fn pose(&mut self, tick: f32, hz: f32, duration: f32, parents: &[i32]) -> Vec<Mat4> {
        let mut delta = tick - self.last_tick;
        if delta < 0.0 {
            delta += duration.max(1.0);
        }
        self.last_tick = tick;
        let dt = (delta / hz.max(1.0)).clamp(0.0, 0.1);
        let mut globals = self.rest.clone();
        let frame = self.driver.step(
            &mut globals,
            parents,
            &self.rest,
            -Vec3::Y,
            self.speed(),
            &self.floor,
            dt,
        );
        let hold = Mat4::from_translation(Vec3::new(-frame.origin.x, -frame.origin.y, 0.0));
        for g in &mut globals {
            *g = hold * *g;
        }
        globals
    }
}

/// The ground gait a pattern's default pack authors for its preview, with its family: the
/// state whose generated gait is a WALK, else the first generated ground gait, else a stand
/// (a pack of flight states alone); `None` when the pack has no generated state at all.
fn pack_gait(pattern: Pattern) -> Option<(GaitKind, LocomotionFamily)> {
    let path = flicker_content::baseline::pattern_dir(pattern)
        .join(format!("{}.pack.json", pattern.name()));
    let pack = flicker_skeletal::state::read_pack(&path).ok()?;
    let generated: Vec<GeneratedGait> = pack
        .state_machine
        .states
        .iter()
        .filter_map(|s| match &s.clip {
            ClipSource::Generated(g) => Some(*g),
            _ => None,
        })
        .collect();
    if generated.is_empty() {
        return None;
    }
    let ground = |g: &GeneratedGait| kind_of(g.gait).filter(|k| *k != GaitKind::Stand);
    let pick = generated
        .iter()
        .find(|g| kind_of(g.gait) == Some(GaitKind::Walk))
        .or_else(|| generated.iter().find(|g| ground(g).is_some()));
    Some(match pick {
        Some(g) => (ground(g).unwrap_or(GaitKind::Stand), g.family.into()),
        None => (GaitKind::Stand, generated[0].family.into()),
    })
}

/// The preview step's subject: the committed bake's skinned mesh, posed by the shared
/// idle clip on the bench's clock — or, for a creature pattern without clips, by the
/// generated walk.
pub(crate) struct BakePreview {
    bones: Vec<SkelBone>,
    pub(crate) parents: Vec<i32>,
    pub(crate) clip: ResolvedClip,
    /// The rig's drawable body — the GPU-skinned mesh PLUS the CPU cloth submesh split off it
    /// when the bake carried cloth regions (spec 6C46CAB9). THE one skinned-upload door.
    body: SkinnedBody,
    /// The source folder's maps (albedo + the PBR set it ships), for the preview's PBR toggle;
    /// `None` when the folder ships no base colour.
    material: Option<(TextureHandle, PbrMaps)>,
    bone_count: u32,
    /// The clip tick the cloth was last stepped to, so the step runs on the preview's OWN clock
    /// rather than on however many frames the bench happened to draw.
    cloth_tick: f32,
    pub(crate) centre: Vec3,
    pub(crate) radius: f32,
    /// The feet plane, absolute.
    pub(crate) floor: f32,
    /// The generated walk when the clip is the trackless "rest" placeholder of a creature
    /// pattern (the driver keeps foot state, so it steps under the bench's shared borrow).
    walker: Option<std::cell::RefCell<GeneratedWalk>>,
}

impl BakePreview {
    /// The pose at `tick` (wrapped to the clip): the bones' globals and the skinning palette.
    pub(crate) fn pose(&self, tick: f32) -> (Vec<Mat4>, Vec<Mat4>) {
        if let Some(walker) = &self.walker {
            let globals = walker.borrow_mut().pose(
                tick,
                self.clip.tick_rate_hz as f32,
                self.clip.duration_ticks as f32,
                &self.parents,
            );
            let palette = skin::palette(&self.bones, &globals);
            return (globals, palette);
        }
        let tick = (tick as u32).min(self.clip.duration_ticks.saturating_sub(1));
        let locals = sample_local_poses(&self.bones, &self.clip, tick, true);
        let globals = global_transforms(&self.bones, &locals);
        let palette = skin::palette(&self.bones, &globals);
        (globals, palette)
    }

    /// The body's draw item: under its source maps when `pbr` is on (and the folder ships
    /// them), the neutral steel otherwise.
    pub(crate) fn draw(&self, palette: Vec<Mat4>, pbr: bool) -> Option<Draw> {
        let material = self.material.filter(|_| pbr);
        self.body
            .draw_with(Mat4::IDENTITY, palette, self.bone_count, material)
    }
}

/// The caches, keyed by the document's generations.
pub(crate) struct ViewMeshes {
    textures: HashMap<PathBuf, TextureHandle>,
    preview: Option<(Uploaded, PreviewKey, u64)>,
    wire: Option<(MeshHandle, (PreviewKey, u64))>,
    /// The SELECTED REGION's own triangles, keyed by the source, the regions' generation and which
    /// row is picked — the wireframe twin's mechanism over a subset (spec 0A81088E T2).
    region: Option<(MeshHandle, (PreviewKey, u64, usize))>,
    skinned: Option<(Uploaded, (PreviewKey, u64))>,
    base: Option<BasePreview>,
    base_upload: Option<Uploaded>,
    bake: Option<BakePreview>,
}

impl ViewMeshes {
    pub(crate) fn new() -> Self {
        Self {
            textures: HashMap::new(),
            preview: None,
            wire: None,
            region: None,
            skinned: None,
            base: None,
            base_upload: None,
            bake: None,
        }
    }

    /// Scene entry: load the fitting body once and upload it when within budget.
    pub(crate) fn enter(&mut self, r: &mut Renderer) {
        if self.base.is_none() {
            self.base = BasePreview::load();
        }
        let Self {
            base,
            textures,
            base_upload,
            ..
        } = self;
        if let (Some(b), None) = (base.as_ref(), base_upload.as_ref()) {
            if !b.verts.is_empty() {
                let up = upload_preview(r, textures, &b.maps, &b.verts, &b.uvs, &b.indices);
                tracing::info!(
                    verts = b.verts.len(),
                    textured = matches!(up, Uploaded::Textured { .. }),
                    "clayworks: fitting body uploaded"
                );
                *base_upload = Some(up);
            }
        }
    }

    /// Scene exit: give every handle back.
    pub(crate) fn free(&mut self, r: &mut Renderer) {
        if let Some((up, _, _)) = self.preview.take() {
            up.free(r);
        }
        if let Some((h, _)) = self.wire.take() {
            r.free_mesh(h);
        }
        if let Some((h, _)) = self.region.take() {
            r.free_mesh(h);
        }
        if let Some((up, _)) = self.skinned.take() {
            up.free(r);
        }
        if let Some(up) = self.base_upload.take() {
            up.free(r);
        }
        self.release_bake(r);
    }

    pub(crate) fn base(&self) -> Option<&BasePreview> {
        self.base.as_ref()
    }

    pub(crate) fn base_upload(&self) -> Option<Uploaded> {
        self.base_upload
    }

    fn key_of(doc: &Document) -> Option<(PreviewKey, bool, bool)> {
        let src = doc.source.as_ref()?;
        let parsed = src.parsed.as_ref();
        let has_mesh = parsed.is_some_and(|p| !p.model.vertices.is_empty());
        let has_bones = parsed.is_some_and(|p| !p.model.bones.is_empty());
        Some(((src.dir.clone(), src.candidate_sel), has_mesh, has_bones))
    }

    /// The source mesh as parsed (textured when its folder ships maps).
    pub(crate) fn source_mesh(&mut self, doc: &Document, r: &mut Renderer) -> Option<Uploaded> {
        let (key, has_mesh, _) = Self::key_of(doc)?;
        if !has_mesh {
            return None;
        }
        let need = match &self.preview {
            Some((_, k, g)) => *k != key || *g != doc.mesh_gen,
            None => true,
        };
        if need {
            if let Some((old, _, _)) = self.preview.take() {
                old.free(r);
            }
            let src = doc.source.as_ref()?;
            let parsed = src.parsed.as_ref()?;
            let verts: Vec<MeshVertex> = parsed
                .model
                .vertices
                .iter()
                .map(|v| MeshVertex {
                    position: v.p,
                    normal: v.n,
                    material: 0,
                })
                .collect();
            let uvs: Vec<[f32; 2]> = parsed.model.vertices.iter().map(|v| v.uv).collect();
            let maps = source_maps(&src.scan, &src.fbx);
            let up = upload_preview(
                r,
                &mut self.textures,
                &maps,
                &verts,
                &uvs,
                &parsed.model.indices,
            );
            self.preview = Some((up, key, doc.mesh_gen));
        }
        self.preview.as_ref().map(|(h, _, _)| *h)
    }

    /// The source mesh's flat twin for the wireframe pass.
    pub(crate) fn wire_mesh(&mut self, doc: &Document, r: &mut Renderer) -> Option<MeshHandle> {
        let (key, has_mesh, _) = Self::key_of(doc)?;
        if !has_mesh {
            return None;
        }
        let want = (key, doc.mesh_gen);
        let need = self.wire.as_ref().is_none_or(|(_, k)| *k != want);
        if need {
            if let Some((old, _)) = self.wire.take() {
                r.free_mesh(old);
            }
            let parsed = doc.source.as_ref()?.parsed.as_ref()?;
            let verts: Vec<MeshVertex> = parsed
                .model
                .vertices
                .iter()
                .map(|v| MeshVertex {
                    position: v.p,
                    normal: v.n,
                    material: 0,
                })
                .collect();
            let h = r.upload_mesh(&verts, MeshIndices::U32(&parsed.model.indices));
            self.wire = Some((h, want));
        }
        self.wire.as_ref().map(|(h, _)| *h)
    }

    /// THE SELECTED REGION, drawn (spec 0A81088E T2): the wireframe twin's very mechanism over the
    /// picked region's own triangles — a triangle belongs when ALL THREE of its corners are members,
    /// so the highlight stops at the region's boundary instead of bleeding across its seam. The
    /// caller tints it from the theme; this only uploads. `None` with no pick or an empty region.
    pub(crate) fn region_mesh(&mut self, doc: &Document, r: &mut Renderer) -> Option<MeshHandle> {
        let (key, has_mesh, _) = Self::key_of(doc)?;
        let sel = doc.region_sel()?;
        if !has_mesh || doc.region_verts().is_empty() {
            return None;
        }
        let want = (key, doc.region_gen, sel);
        if self.region.as_ref().is_none_or(|(_, k)| *k != want) {
            if let Some((old, _)) = self.region.take() {
                r.free_mesh(old);
            }
            let parsed = doc.source.as_ref()?.parsed.as_ref()?;
            let mut member = vec![false; parsed.model.vertices.len()];
            for &v in doc.region_verts() {
                if let Some(m) = member.get_mut(v as usize) {
                    *m = true;
                }
            }
            let verts: Vec<MeshVertex> = parsed
                .model
                .vertices
                .iter()
                .map(|v| MeshVertex {
                    position: v.p,
                    normal: v.n,
                    material: 0,
                })
                .collect();
            let indices: Vec<u32> = parsed
                .model
                .indices
                .as_chunks::<3>()
                .0
                .iter()
                .filter(|t| t.iter().all(|&i| member[i as usize]))
                .flatten()
                .copied()
                .collect();
            if indices.is_empty() {
                return None;
            }
            let h = r.upload_mesh(&verts, MeshIndices::U32(&indices));
            self.region = Some((h, want));
        }
        self.region.as_ref().map(|(h, _)| *h)
    }

    /// The rigged source CPU-skinned to its current pose (re-skinned when the pose moves).
    pub(crate) fn skinned_mesh(&mut self, doc: &Document, r: &mut Renderer) -> Option<Uploaded> {
        let (key, has_mesh, has_bones) = Self::key_of(doc)?;
        if !has_mesh || !has_bones {
            return None;
        }
        let want = (key, doc.pose_gen);
        let need = self.skinned.as_ref().is_none_or(|(_, k)| *k != want);
        if need {
            if let Some((old, _)) = self.skinned.take() {
                old.free(r);
            }
            let src = doc.source.as_ref()?;
            let parsed = src.parsed.as_ref()?;
            let verts = skin_source_verts(&parsed.model, &parsed.globals);
            let uvs: Vec<[f32; 2]> = parsed.model.vertices.iter().map(|v| v.uv).collect();
            let maps = source_maps(&src.scan, &src.fbx);
            let up = upload_preview(
                r,
                &mut self.textures,
                &maps,
                &verts,
                &uvs,
                &parsed.model.indices,
            );
            self.skinned = Some((up, want));
        }
        self.skinned.as_ref().map(|(h, _)| *h)
    }

    /// The bake preview, built once from the document's bake parts (an error lands on the
    /// document's status line, once).
    pub(crate) fn bake(&mut self, doc: &mut Document, r: &mut Renderer) -> Option<&BakePreview> {
        if self.bake.is_none() {
            match doc.bake_preview_parts() {
                Ok((rig_file, bones, clip)) => {
                    let rest: Vec<Mat4> = bones.iter().map(|b| b.local).collect();
                    let globals = global_transforms(&bones, &rest);
                    let mut min = Vec3::splat(f32::MAX);
                    let mut max = Vec3::splat(f32::MIN);
                    for g in &globals {
                        let p = g.w_axis.truncate();
                        min = min.min(p);
                        max = max.max(p);
                    }
                    let centre = (min + max) * 0.5;
                    let radius = ((max - min).length() * 0.5).max(1.0);
                    let floor = min.z;
                    let body = SkinnedBody::upload(r, &rig_file.mesh, &bones, &rig_file.collision);
                    // The source folder's maps, through the same cache the source preview
                    // fills (albedo sRGB, the data maps linear).
                    let material = doc.source.as_ref().and_then(|src| {
                        let maps = source_maps(&src.scan, &src.fbx);
                        let albedo =
                            load_map(r, &mut self.textures, maps.base_color.as_deref()?, true)?;
                        let mut linear = |p: Option<&Path>| {
                            p.and_then(|p| load_map(r, &mut self.textures, p, false))
                        };
                        Some((
                            albedo,
                            PbrMaps {
                                normal: linear(maps.normal.as_deref()),
                                roughness: linear(maps.roughness.as_deref()),
                                metalness: linear(maps.metalness.as_deref()),
                                ..Default::default()
                            },
                        ))
                    });
                    let bone_count = bones.len() as u32;
                    let parents: Vec<i32> = bones.iter().map(|b| b.parent).collect();
                    let walker = clip
                        .tracks
                        .is_empty()
                        .then(|| GeneratedWalk::new(&bones, doc.recipe().pattern()))
                        .flatten()
                        .map(std::cell::RefCell::new);
                    tracing::info!(
                        bones = bones.len(),
                        verts = rig_file.mesh.vertices.len(),
                        cloth = body.has_cloth(),
                        pbr = material.is_some(),
                        clip = %clip.name,
                        generated_walk = walker.is_some(),
                        "clayworks: bake preview built"
                    );
                    self.bake = Some(BakePreview {
                        bones,
                        parents,
                        clip,
                        body,
                        material,
                        bone_count,
                        cloth_tick: 0.0,
                        centre,
                        radius,
                        floor,
                        walker,
                    });
                }
                Err(e) => {
                    if let Some(s) = doc.source.as_mut() {
                        if s.error.as_deref() != Some(e.as_str()) {
                            tracing::warn!("clayworks: bake preview: {e}");
                            s.error = Some(e);
                        }
                    }
                }
            }
        }
        self.bake.as_ref()
    }

    /// The bake preview if it has been built (no ensure — `update` reads it for the pose).
    pub(crate) fn bake_ref(&self) -> Option<&BakePreview> {
        self.bake.as_ref()
    }

    /// Drop the bake preview (leaving the preview step, or the document changed).
    pub(crate) fn release_bake(&mut self, r: &mut Renderer) {
        if let Some(mut bp) = self.bake.take() {
            bp.body.free(r);
        }
    }

    /// Step the bake preview's CLOTH to `tick` and rewrite its vertex buffer — the live half of
    /// the runtime-cloth path (spec 6C46CAB9). `dt` comes from the preview's OWN clip tick, not
    /// from the frame, so the drape is the same at any frame rate. A bake with no cloth, or a
    /// pose that has not been computed yet, costs nothing.
    pub(crate) fn step_bake_cloth(&mut self, r: &mut Renderer, palette: &[Mat4], tick: f32) {
        let Some(bp) = self.bake.as_mut() else { return };
        if palette.is_empty() || !bp.body.has_cloth() {
            return;
        }
        let hz = bp.clip.tick_rate_hz.max(1) as f32;
        let dt = (tick - bp.cloth_tick) / hz;
        bp.cloth_tick = tick;
        // A clip WRAP (or a scrub the human made) is not elapsed time: integrating it would fling
        // the cloth, and re-settling it would hitch once per loop. Keep the drape it has.
        if dt <= 0.0 || dt >= 0.25 {
            return;
        }
        bp.body.pose(r, palette, Some(dt));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// THE PREVIEW WALKS A CREATURE (G6 of the gait/IK design): the packaged Quadruped's
    /// generated walk, stepped two seconds on the bench's 60 Hz clock, holds the body over the
    /// origin, keeps every hoof on or above its floor, lifts hooves in turn, and never asks a
    /// hoof for more than its leg reaches; a humanoid pattern gets no walker. Real content;
    /// skips without it.
    #[test]
    fn the_preview_walks_the_packaged_quadruped_on_a_treadmill() {
        use flicker_skeletal::format::{rig_bones, RigFile};
        let path =
            flicker_content::baseline::pattern_dir(Pattern::Quadruped).join("Quadruped.json");
        let Ok(text) = flicker_content::package::read_text(&path) else {
            eprintln!("skipping: no packaged quadruped");
            return;
        };
        let file: RigFile = serde_json::from_str(&text).expect("the packaged quadruped parses");
        let bones = rig_bones(&file);
        let parents: Vec<i32> = bones.iter().map(|b| b.parent).collect();
        assert!(
            GeneratedWalk::new(&bones, Pattern::Humanoid).is_none(),
            "humanoids animate from clips"
        );
        let mut walk = GeneratedWalk::new(&bones, Pattern::Quadruped).expect("a quadruped walks");
        assert_eq!(walk.kind, GaitKind::Walk, "the pack's walk");
        assert_eq!(
            pack_gait(Pattern::Bat).map(|(k, _)| k),
            Some(GaitKind::Stand),
            "a pack of flight states alone previews standing"
        );
        assert!(
            walk.speed() > 50.0 && walk.speed() < 300.0,
            "{} cm/s",
            walk.speed()
        );
        let floor = walk.floor.height;
        let hooves: Vec<usize> = walk.driver.feet.iter().map(|f| f.limb.effector).collect();
        assert_eq!(hooves.len(), 4);
        let mut lifted = [false; 4];
        let mut moved = false;
        let mut first = None;
        for tick in 0..120u32 {
            let globals = walk.pose((tick % 60) as f32, 60.0, 60.0, &parents);
            let root = globals[0].w_axis.truncate();
            assert!(
                root.x.abs() < 1e-3 && root.y.abs() < 1e-3,
                "held over the origin: {root}"
            );
            // At the walk's full speed (1.56 m/s on this horse) the straight foreleg's stance
            // extremes leave the solver a few millimetres short for a tick — invisible, and
            // the 1 m/s bench gate holds it under half a centimetre.
            assert!(
                walk.driver.residual < 1.0,
                "reach: {} at {tick}",
                walk.driver.residual
            );
            for (k, &h) in hooves.iter().enumerate() {
                let z = globals[h].w_axis.z;
                assert!(z > floor - 0.5, "a hoof under the floor: {z} at {tick}");
                if z > floor + 2.0 {
                    lifted[k] = true;
                }
            }
            let snapshot: Vec<Vec3> = hooves
                .iter()
                .map(|&h| globals[h].w_axis.truncate())
                .collect();
            match &first {
                None => first = Some(snapshot),
                Some(f) => {
                    if f.iter()
                        .zip(&snapshot)
                        .any(|(a, b)| (*a - *b).length() > 1.0)
                    {
                        moved = true;
                    }
                }
            }
        }
        assert!(
            lifted.iter().all(|l| *l),
            "every hoof lifts in turn: {lifted:?}"
        );
        assert!(moved, "the pose changes over the walk");
    }
}
