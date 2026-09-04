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
    Renderer, SkinnedMeshHandle, SkinnedVertex, TextureHandle, TexturedMeshHandle, Vec3,
};
use flicker_content::{attach_world, fitting_base, source_maps, Fit, SourceMaps};
use flicker_rigview::Draw;
use flicker_skeletal::format::{Bone as SkelBone, ResolvedClip};
use flicker_skeletal::pose::{global_transforms, sample_local_poses};
use flicker_skeletal::skin;

use crate::services::{skin_source_verts, Document, PropFit, SOCKETS};

/// The fitting body (GolemBase) is a dense mesh; above this many vertices the reference
/// view shows its skeleton only — a fit needs the joints, not a 50 MB body.
pub(crate) const BASE_MESH_BUDGET: usize = 150_000;

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

/// The preview step's subject: the committed bake's skinned mesh, posed by the shared
/// idle clip on the bench's clock.
pub(crate) struct BakePreview {
    bones: Vec<SkelBone>,
    pub(crate) parents: Vec<i32>,
    pub(crate) clip: ResolvedClip,
    mesh: SkinnedMeshHandle,
    bone_count: u32,
    pub(crate) centre: Vec3,
    pub(crate) radius: f32,
    /// The feet plane, absolute.
    pub(crate) floor: f32,
}

impl BakePreview {
    /// The pose at `tick` (wrapped to the clip): the bones' globals and the skinning palette.
    pub(crate) fn pose(&self, tick: f32) -> (Vec<Mat4>, Vec<Mat4>) {
        let tick = (tick as u32).min(self.clip.duration_ticks.saturating_sub(1));
        let locals = sample_local_poses(&self.bones, &self.clip, tick, true);
        let globals = global_transforms(&self.bones, &locals);
        let palette = skin::palette(&self.bones, &globals);
        (globals, palette)
    }

    pub(crate) fn draw(&self, palette: Vec<Mat4>) -> Draw {
        Draw::Skinned {
            mesh: self.mesh,
            world: Mat4::IDENTITY,
            palette,
            bone_count: self.bone_count,
        }
    }
}

/// The caches, keyed by the document's generations.
pub(crate) struct ViewMeshes {
    textures: HashMap<PathBuf, TextureHandle>,
    preview: Option<(Uploaded, PreviewKey, u64)>,
    wire: Option<(MeshHandle, (PreviewKey, u64))>,
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
                    let verts: Vec<SkinnedVertex> = rig_file
                        .mesh
                        .vertices
                        .iter()
                        .map(|v| SkinnedVertex {
                            position: v.p,
                            normal: v.n,
                            uv: v.uv,
                            joints: v.joints,
                            weights: v.weights,
                        })
                        .collect();
                    let indices: Vec<u32> = if rig_file.mesh.indices.is_empty() {
                        (0..verts.len() as u32).collect()
                    } else {
                        rig_file.mesh.indices.clone()
                    };
                    let mesh = r.upload_skinned_mesh(&verts, MeshIndices::U32(&indices));
                    let bone_count = bones.len() as u32;
                    tracing::info!(
                        bones = bones.len(),
                        verts = verts.len(),
                        clip = %clip.name,
                        "clayworks: bake preview built"
                    );
                    let parents: Vec<i32> = bones.iter().map(|b| b.parent).collect();
                    self.bake = Some(BakePreview {
                        bones,
                        parents,
                        clip,
                        mesh,
                        bone_count,
                        centre,
                        radius,
                        floor,
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
        if let Some(bp) = self.bake.take() {
            r.free_skinned_mesh(bp.mesh);
        }
    }
}
