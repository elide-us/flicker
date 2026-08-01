//! Bake a conformed [`RawModel`] to the canonical `flicker.rig` — the last stage of the in-app
//! pipeline (WS-F slice 4). Emits the SAME JSON the Blender exporter produces, via the canonical
//! `flicker-skeletal` format types (their `Serialize` derive is additive), so there is one schema.
//!
//! Two things happen here that the conform deliberately left out, because they are bake concerns:
//!   * a synthesized identity **`root`** bone at index 0 (Meshy rigs have none; the engine + baked
//!     clips expect `root` at the feet as bone 0, `pelvis` parented to it) — every existing parent
//!     index and every vertex joint index shifts +1;
//!   * the source header (`Z_up` / `cm` / `applied_transform: none`) the loader reads to orient the
//!     rig into engine space, and `retarget: true` (rotation-only playback for a retargeted body).

use anyhow::{bail, Context, Result};
use glam::{EulerRot, Mat3, Mat4, Quat, Vec3};
use std::collections::HashMap;
use std::path::Path;

use flicker_skeletal::format::{
    Attach, BoneRaw, Material, Mesh, RigFile, Skeleton, Source, Submesh, Vertex,
};

use crate::fbx::RawModel;

const IDENTITY16: [f32; 16] = [
    1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
];

/// Assemble a [`RigFile`] from a conformed model. `source_name` is the asset stem (the exporter's
/// `--out` basename, e.g. `"PrismHumanBaseA"`). A single flat material/submesh is emitted when the
/// model carries no material info; textured materials are wired in the texture slice.
pub fn bake_rig(model: &RawModel, source_name: &str) -> RigFile {
    // Synthesized identity root at bone 0; every real bone's parent shifts +1 (a former root → 0).
    let mut bones = Vec::with_capacity(model.bones.len() + 1);
    bones.push(BoneRaw {
        name: "root".to_string(),
        parent: -1,
        local: IDENTITY16,
        inverse_bind: IDENTITY16,
    });
    for b in &model.bones {
        let local = Mat4::from_scale_rotation_translation(
            Vec3::from(b.scale),
            Quat::from_array(b.rotation),
            Vec3::from(b.translation),
        )
        .to_cols_array();
        bones.push(BoneRaw {
            name: b.name.clone(),
            parent: if b.parent < 0 { 0 } else { b.parent + 1 },
            local,
            inverse_bind: b.inverse_bind,
        });
    }

    // Vertices: the +1 root shift moves every joint index up by one (0-weight pads included — inert).
    // EXCEPT when the source carried no bones at all: then the synthesized `root` is the ONLY bone,
    // and shifting would point every vertex at a non-existent bone 1. Such a mesh belongs in
    // [`bake_prop`], but the character path must still emit a VALID file if one reaches it.
    let shift = if model.bones.is_empty() { 0 } else { 1 };
    let vertices: Vec<Vertex> = model
        .vertices
        .iter()
        .map(|v| Vertex {
            p: v.p,
            n: v.n,
            uv: v.uv,
            joints: [
                v.joints[0] + shift,
                v.joints[1] + shift,
                v.joints[2] + shift,
                v.joints[3] + shift,
            ],
            weights: v.weights,
        })
        .collect();
    let indices = model.indices.clone();

    // One flat submesh/material for now (untextured → the loader renders it neutral).
    let materials = vec![Material {
        name: "material_0".to_string(),
        slot: "material_0".to_string(),
        ..Default::default()
    }];
    let submeshes = vec![Submesh { material: 0, start: 0, count: indices.len() }];

    RigFile {
        format: "flicker.rig".to_string(),
        version: 1,
        source: Source {
            file: source_name.to_string(),
            source_axis: "Z_up".to_string(),
            source_unit: "cm".to_string(),
            applied_transform: "none".to_string(),
            ..Default::default()
        },
        skeleton: Skeleton { bones },
        mesh: Mesh { vertices, indices, submeshes, materials, ..Default::default() },
        clips: Vec::new(),
        attach: Default::default(),
        collision: Default::default(),
        retarget: true,
    }
}

/// Bake a bone-less static **PROP** mesh (a weapon, an accessory, a raw garment before it is
/// skinned) to `flicker.rig`. A prop is the OPPOSITE of the character bake:
///   * it is NEVER skinned and carries **no skeleton** (`skeleton.bones: []`) — so there is no
///     synthesized `root` and **no `+1` joint shift**; the vertices, whose skin is the inert
///     `([0;4], [0.0;4])` `parse_fbx` yields for an unrigged mesh, pass through verbatim;
///   * `retarget` is **false** — a rigid prop plays no retargeted clips;
///   * the socket it hangs from and its fit (offset/rotate/scale) are NOT baked here — they are
///     authored in the editor and folded into the `attach` block from `fits.json` at load time
///     (`flicker-paperdoll::write_inline_attach`), which is why `export_prop` also omits them.
///
/// Byte-shape parity with the Python `io_scene_flicker_rig.py::export_prop`.
/// Recompute the mesh's skin WEIGHTS from the current skeleton — the in-app replacement for a
/// source rig's (often poor) auto-skinning. Each vertex is bound to its nearest bone SEGMENTS
/// (joint→first-child) by inverse-square distance, top-4, normalised. Run AFTER the bones are
/// repositioned inside the mesh: the rest pose and `inverse_bind` are untouched, so this changes only
/// how the mesh DEFORMS when posed, never where it sits at rest.
pub fn bake_skin(model: &mut RawModel) {
    let n = model.bones.len();
    if n == 0 || model.vertices.is_empty() {
        return;
    }
    let globals = rest_world_frames(model);
    let heads: Vec<Vec3> = globals.iter().map(|g| g.w_axis.truncate()).collect();
    // A bone's body = the segment from its joint to its FIRST child's joint; a leaf is a point.
    let tails: Vec<Vec3> = (0..n)
        .map(|i| {
            model
                .bones
                .iter()
                .position(|b| b.parent == i as i32)
                .map(|c| heads[c])
                .unwrap_or(heads[i])
        })
        .collect();
    for v in &mut model.vertices {
        let p = Vec3::from_array(v.p);
        let mut scored: Vec<(usize, f32)> =
            (0..n).map(|i| (i, dist_point_segment(p, heads[i], tails[i]))).collect();
        scored.sort_by(|a, b| a.1.total_cmp(&b.1));
        let mut joints = [0u32; 4];
        let mut weights = [0.0f32; 4];
        let mut sum = 0.0;
        for (k, &(bi, dist)) in scored.iter().take(4).enumerate() {
            let w = 1.0 / (dist * dist + 1e-3);
            joints[k] = bi as u32;
            weights[k] = w;
            sum += w;
        }
        if sum > 0.0 {
            for w in &mut weights {
                *w /= sum;
            }
        }
        v.joints = joints;
        v.weights = weights;
    }
}

/// Rest WORLD frame per bone, composed from the stored local TRS (parents precede children).
fn rest_world_frames(model: &RawModel) -> Vec<Mat4> {
    let mut g: Vec<Mat4> = Vec::with_capacity(model.bones.len());
    for b in &model.bones {
        let local = Mat4::from_scale_rotation_translation(
            Vec3::from_array(b.scale),
            Quat::from_array(b.rotation),
            Vec3::from_array(b.translation),
        );
        let world = match usize::try_from(b.parent) {
            Ok(p) if p < g.len() => g[p] * local,
            _ => local,
        };
        g.push(world);
    }
    g
}

/// Shortest distance from point `p` to the segment `a`–`b`.
fn dist_point_segment(p: Vec3, a: Vec3, b: Vec3) -> f32 {
    let ab = b - a;
    let len2 = ab.length_squared();
    let t = if len2 > 1e-12 { ((p - a).dot(ab) / len2).clamp(0.0, 1.0) } else { 0.0 };
    (p - (a + ab * t)).length()
}

pub fn bake_prop(model: &RawModel, source_name: &str) -> RigFile {
    let vertices: Vec<Vertex> = model
        .vertices
        .iter()
        .map(|v| Vertex {
            p: v.p,
            n: v.n,
            uv: v.uv,
            joints: v.joints,
            weights: v.weights,
        })
        .collect();
    let indices = model.indices.clone();
    // One flat submesh/material — the same placeholder the character bake emits, over which
    // [`write_prop`] wires the source folder's maps. Left un-textured (a bake straight from
    // `bake_prop`, with no folder to read) the paperdoll renders the prop as flat steel.
    let materials = vec![Material {
        name: "material_0".to_string(),
        slot: "material_0".to_string(),
        ..Default::default()
    }];
    let submeshes = vec![Submesh { material: 0, start: 0, count: indices.len() }];

    RigFile {
        format: "flicker.rig".to_string(),
        version: 1,
        source: Source {
            file: source_name.to_string(),
            source_axis: "Z_up".to_string(),
            source_unit: "cm".to_string(),
            applied_transform: "none".to_string(),
            ..Default::default()
        },
        skeleton: Skeleton { bones: Vec::new() },
        mesh: Mesh { vertices, indices, submeshes, materials, ..Default::default() },
        clips: Vec::new(),
        attach: Default::default(),
        collision: Default::default(),
        retarget: false,
    }
}

/// Bake a raw garment mesh SKINNED onto a body — the outfit-overlay path (WS-F slice 8, porting
/// `tools/skin_outfit.py`). Unlike a prop (drawn rigid, fit applied live at draw), a garment is
/// **skinned by the body's palette and its fit is BAKED into the vertex positions**, then drawn at
/// the plain `world` matrix. So this:
///   1. rebuilds the socket's placement transform EXACTLY as the engine's `PieceFit::matrix`
///      (`flicker-paperdoll`): `world = rest_global[socket] · from_quat(rot) · SRT(scale·uniform,
///      user_rot, offset)`, where `rot` cancels the socket's rest rotation (its `inverse_bind`
///      rotation part), so `offset` acts along world axes at rest;
///   2. bakes every garment vertex through `world` (positions; inverse-transpose for normals);
///   3. transfers skin from the NEAREST body vertex (its `joints`+`weights`, already root-shifted
///      and normalised) — a garment has no skeleton of its own;
///   4. emits the body's FULL `skeleton.bones` verbatim, so `load_outfit`'s by-name remap onto the
///      base is the identity, and marks `applied_transform:"baked-fit"`, `retarget:false`.
///
/// `fit.rotate` is consumed as a QUATERNION (the engine's `user_rot`), not re-Eulerised.
pub fn bake_garment(
    garment: &RawModel,
    source_name: &str,
    body: &RigFile,
    socket: &str,
    fit: &Attach,
) -> Result<RigFile> {
    let socket_idx = body
        .skeleton
        .bones
        .iter()
        .position(|b| b.name == socket)
        .with_context(|| format!("socket bone {socket:?} is not in the body skeleton"))?;
    if body.mesh.vertices.is_empty() {
        bail!("the body rig carries no mesh — nothing to transfer skin weights from");
    }

    // The socket placement — SHARED with the editor's viewport preview via `attach_world`, so what
    // the user positions on screen is byte-for-byte what bakes here (no fit-math drift, gotcha #4).
    let world = attach_world(&body.skeleton.bones[socket_idx].inverse_bind, fit);
    let normal_mat = Mat3::from_mat4(world).inverse().transpose();

    let body_verts = &body.mesh.vertices;
    // Spatial hash over the body once, so a dense base does not turn the transfer quadratic.
    let grid = VertexGrid::build(body_verts);
    let vertices: Vec<Vertex> = garment
        .vertices
        .iter()
        .map(|v| {
            let p = world.transform_point3(Vec3::from(v.p));
            let n = (normal_mat * Vec3::from(v.n)).normalize_or_zero();
            let nearest = grid.nearest(p, body_verts);
            Vertex {
                p: p.to_array(),
                n: n.to_array(),
                uv: v.uv,
                joints: body_verts[nearest].joints,
                weights: body_verts[nearest].weights,
            }
        })
        .collect();

    let indices = garment.indices.clone();
    let materials = vec![Material {
        name: "material_0".to_string(),
        slot: "material_0".to_string(),
        ..Default::default()
    }];
    let submeshes = vec![Submesh { material: 0, start: 0, count: indices.len() }];

    Ok(RigFile {
        format: "flicker.rig".to_string(),
        version: 1,
        source: Source {
            file: format!("{source_name} (skinned to body)"),
            source_axis: "Z_up".to_string(),
            source_unit: "cm".to_string(),
            applied_transform: "baked-fit".to_string(),
            ..Default::default()
        },
        skeleton: Skeleton { bones: body.skeleton.bones.clone() },
        mesh: Mesh { vertices, indices, submeshes, materials, ..Default::default() },
        clips: Vec::new(),
        attach: Default::default(),
        collision: Default::default(),
        retarget: false,
    })
}

/// The world placement an [`Attach`] resolves to on a socket bone, given the bone's `inverse_bind`
/// (= inverse rest world): `rest_global · from_quat(rot) · SRT(scale·uniform, user_rot, offset)`,
/// where `rot` cancels the socket's rest rotation so `offset` acts along world axes. This IS the
/// engine's `PieceFit::matrix` (flicker-paperdoll); the garment bake AND the editor's viewport
/// preview both call it, so the placement the user approves on screen is exactly what bakes.
pub fn attach_world(socket_inverse_bind: &[f32; 16], attach: &Attach) -> Mat4 {
    let inv_bind = Mat4::from_cols_array(socket_inverse_bind);
    let rest_global = inv_bind.inverse();
    let rot = Quat::from_mat4(&inv_bind).normalize();
    rest_global
        * Mat4::from_quat(rot)
        * Mat4::from_scale_rotation_translation(
            Vec3::from(attach.scale) * attach.uniform,
            Quat::from_array(attach.rotate).normalize(),
            Vec3::from(attach.offset),
        )
}

/// A uniform spatial hash over the body vertices for the weight transfer.
///
/// `skin_outfit.py` used exact brute force deliberately — a garment hangs loose, so a *capped*
/// search can silently grab the wrong side of a fold. This keeps that EXACTNESS (it returns the
/// same vertex brute force would) while dropping the cost: the expanding-shell scan stops only once
/// the nearest cell boundary is farther than the best hit so far, which is a sound bound. Needed
/// because a dense base (GolemBase carries ~285k verts vs the human base's ~12.5k) turns
/// O(garment · body) into ~70 s per piece.
struct VertexGrid {
    cell: f32,
    buckets: HashMap<(i32, i32, i32), Vec<u32>>,
}

impl VertexGrid {
    fn build(verts: &[Vertex]) -> Self {
        let (mut lo, mut hi) = (Vec3::splat(f32::INFINITY), Vec3::splat(f32::NEG_INFINITY));
        for v in verts {
            let p = Vec3::from(v.p);
            lo = lo.min(p);
            hi = hi.max(p);
        }
        // Aim for ~1 vertex per cell: side ≈ extent / cbrt(n).
        let extent = (hi - lo).max_element().max(1.0);
        let cell = (extent / (verts.len() as f32).cbrt().max(1.0)).max(0.25);
        let mut buckets: HashMap<(i32, i32, i32), Vec<u32>> = HashMap::new();
        for (i, v) in verts.iter().enumerate() {
            buckets.entry(cell_key(Vec3::from(v.p), cell)).or_default().push(i as u32);
        }
        Self { cell, buckets }
    }

    /// The index of the body vertex closest to `p` — identical to a brute-force argmin.
    fn nearest(&self, p: Vec3, verts: &[Vertex]) -> usize {
        // A few shells cover any query sitting on or near the body. A vertex far outside the grid
        // (a garment not yet fitted) would otherwise walk hundreds of empty shells, so past this we
        // just do the exact scan — bounded, and never worse than brute force.
        const MAX_SHELLS: i32 = 6;
        let c = cell_key(p, self.cell);
        let (mut best, mut best_d) = (0usize, f32::INFINITY);
        for r in 0..=MAX_SHELLS {
            self.scan_shell(c, r, p, verts, &mut best, &mut best_d);
            // Anything unscanned sits at Chebyshev distance ≥ r+1 cells, i.e. at least `r * cell`
            // away — so once that bound exceeds the best hit, no closer vertex can exist.
            let bound = r as f32 * self.cell;
            if best_d.is_finite() && bound * bound >= best_d {
                return best;
            }
        }
        let (mut b, mut bd) = (0usize, f32::INFINITY);
        for (i, v) in verts.iter().enumerate() {
            let d = (Vec3::from(v.p) - p).length_squared();
            if d < bd {
                bd = d;
                b = i;
            }
        }
        b
    }

    /// Visit only the SURFACE cells at Chebyshev radius `r` — O(r²) per shell, where iterating the
    /// whole cube and skipping the interior would be O(r³) (and O(r⁴) across the expansion).
    fn scan_shell(
        &self,
        c: (i32, i32, i32),
        r: i32,
        p: Vec3,
        verts: &[Vertex],
        best: &mut usize,
        best_d: &mut f32,
    ) {
        let mut visit = |k: (i32, i32, i32)| {
            let Some(b) = self.buckets.get(&k) else { return };
            for &i in b {
                let d = (Vec3::from(verts[i as usize].p) - p).length_squared();
                if d < *best_d {
                    *best_d = d;
                    *best = i as usize;
                }
            }
        };
        if r == 0 {
            visit(c);
            return;
        }
        for dy in -r..=r {
            for dz in -r..=r {
                visit((c.0 - r, c.1 + dy, c.2 + dz));
                visit((c.0 + r, c.1 + dy, c.2 + dz));
            }
        }
        for dx in -r + 1..=r - 1 {
            for dz in -r..=r {
                visit((c.0 + dx, c.1 - r, c.2 + dz));
                visit((c.0 + dx, c.1 + r, c.2 + dz));
            }
        }
        for dx in -r + 1..=r - 1 {
            for dy in -r + 1..=r - 1 {
                visit((c.0 + dx, c.1 + dy, c.2 - r));
                visit((c.0 + dx, c.1 + dy, c.2 + r));
            }
        }
    }
}

fn cell_key(p: Vec3, cell: f32) -> (i32, i32, i32) {
    (
        (p.x / cell).floor() as i32,
        (p.y / cell).floor() as i32,
        (p.z / cell).floor() as i32,
    )
}

/// Bake `model` (a character) and write the `flicker.rig` JSON to `out`.
///
/// `source_fbx` is the mesh file the model was parsed from — its folder holds the vendor's texture
/// maps, which are brought along by [`wire_source_textures`] exactly as [`write_prop`] and
/// [`write_garment`] do. Without it a character committed through the EDITOR shipped untextured
/// while the CLI [`import_folder`](crate::pipeline::import_folder) path wired its maps — the same
/// gap props carried until they were routed through here.
pub fn write_rig(model: &RawModel, source_fbx: &Path, source_name: &str, out: &Path) -> Result<()> {
    let mut rig = bake_rig(model, source_name);
    wire_source_textures(source_fbx, source_name, out, &mut rig)?;
    write_rig_file(&rig, out)
}

/// Serialize an already-baked [`RigFile`] to `out` — the shared writer the character, prop and
/// garment bakes all funnel through, so there is one JSON-write path (and one place that owns the
/// error context).
pub fn write_rig_file(rig: &RigFile, out: &Path) -> Result<()> {
    let json = serde_json::to_string(rig).context("serializing the rig")?;
    std::fs::write(out, json).with_context(|| format!("writing {}", out.display()))?;
    Ok(())
}

/// A human-authored placement for a prop or garment — the socket bone it mounts to, plus the
/// offset / rotation (euler degrees, applied XYZ) / per-axis scale + scale-all the fit stage tunes.
/// The EDITOR authors this visually and hands it to the bake; it is converted to the format
/// [`Attach`] here so callers never touch the `flicker-skeletal` types. This is the "human in the
/// loop" contract: the tool infers a starting fit, the user corrects it, and the bake honours it.
#[derive(Debug, Clone)]
pub struct Fit {
    pub socket: String,
    pub offset: [f32; 3],
    pub rot_deg: [f32; 3],
    /// PER-AXIS scale of the raw vendor mesh — the X / Y / Z sliders, for a piece that needs
    /// reshaping (a blade lengthened without being thickened).
    pub scale: [f32; 3],
    /// SCALE-ALL, multiplied onto every axis — resize without reshaping. Kept separate from
    /// `scale` so "make it 10% bigger" stays one slider instead of three kept in sync.
    pub uniform: f32,
}

impl Default for Fit {
    fn default() -> Self {
        Self {
            socket: String::new(),
            offset: [0.0; 3],
            rot_deg: [0.0; 3],
            scale: [1.0; 3],
            uniform: 1.0,
        }
    }
}

impl Fit {
    /// The format `Attach` this fit bakes to. Rotation is euler-XYZ degrees → quaternion (the
    /// engine's `user_rot`); per-axis `scale` and scale-all `uniform` map STRAIGHT across, because
    /// the format already carries both and `attach_world` already applies `scale · uniform` — so
    /// widening the editor needed no format change.
    ///
    /// Every factor is floored at 0.001: the sliders can reach zero, and a zero (or negative) axis
    /// collapses the mesh into a plane the bake cannot recover from.
    pub fn to_attach(&self) -> Attach {
        Attach {
            socket: self.socket.clone(),
            offset: self.offset,
            rotate: Quat::from_euler(
                EulerRot::XYZ,
                self.rot_deg[0].to_radians(),
                self.rot_deg[1].to_radians(),
                self.rot_deg[2].to_radians(),
            )
            .to_array(),
            scale: self.scale.map(|s| s.max(0.001)),
            uniform: self.uniform.max(0.001),
        }
    }
}

/// Bake a static prop and write it, folding the editor's authored [`Fit`] into the rig's `attach`
/// block so the paperdoll loads it PRE-FITTED (the socket + offset/rotation/scale the user tuned in
/// the viewport). Editor-facing: [`RawModel`] + paths + `Fit`, never the `flicker-skeletal` types.
///
/// `source_fbx` is the mesh file the model was parsed from — its folder holds the vendor's texture
/// maps, which are brought along by [`wire_source_textures`] exactly as the character import does.
pub fn write_prop(model: &RawModel, source_fbx: &Path, source_name: &str, out: &Path, fit: &Fit) -> Result<()> {
    let mut rig = bake_prop(model, source_name);
    rig.attach = fit.to_attach();
    wire_source_textures(source_fbx, source_name, out, &mut rig)?;
    write_rig_file(&rig, out)
}

/// Bake a garment SKINNED onto the canonical base body using the editor's authored [`Fit`], and
/// write it. The fit's socket + placement position the raw mesh into body bind space BEFORE the
/// nearest-vertex weight transfer, so what the user approved in the viewport is exactly what bakes.
/// Skins onto [`fitting_base`] — the clay Golem fitting body, not the conform canon.
///
/// `source_fbx` carries the garment's own texture maps along, as for [`write_prop`] — a garment
/// takes the BODY's skin weights but keeps its OWN material.
pub fn write_garment(model: &RawModel, source_fbx: &Path, source_name: &str, out: &Path, fit: &Fit) -> Result<()> {
    let body_path = fitting_base();
    let body_text = std::fs::read_to_string(&body_path).with_context(|| {
        format!("reading the fitting body rig {} to skin the garment onto", body_path.display())
    })?;
    let body: RigFile = serde_json::from_str(&body_text).context("parsing the base body rig")?;
    let mut rig = bake_garment(model, source_name, &body, &fit.socket, &fit.to_attach())?;
    wire_source_textures(source_fbx, source_name, out, &mut rig)?;
    write_rig_file(&rig, out)
}

/// Bring the source folder's texture maps along with a prop / garment bake — the SAME wiring the
/// character import runs ([`crate::pipeline::wire_textures`]), reached through the same
/// [`scan`](crate::scan) that classified the folder in the first place. The maps are copied beside
/// the rig as `<AssetName>_<Map>.png` and the material points at those basenames; a folder with no
/// maps simply wires none. Returns what was written.
fn wire_source_textures(
    source_fbx: &Path,
    source_name: &str,
    out: &Path,
    rig: &mut RigFile,
) -> Result<Vec<String>> {
    let folder = crate::pipeline::dir_of(source_fbx);
    let scan = crate::scan::scan_folder(folder)
        .with_context(|| format!("scanning {} for the asset's texture maps", folder.display()))?;
    let out_dir = crate::pipeline::dir_of(out);
    std::fs::create_dir_all(out_dir).with_context(|| format!("creating {}", out_dir.display()))?;
    crate::pipeline::wire_textures(&scan, source_fbx, out_dir, source_name, rig)
}

/// The BODY a garment skins onto, and the body the editor's prop/garment preview mounts against.
///
/// DISTINCT from [`default_reference`](crate::conform), which is the CONFORM canon — the 66-bone
/// skeleton every character is mapped onto. This is the *fitting* body: the clay Golem built for
/// hanging outfits and props on.
///
/// Prefers **`GolemBase_Low`** — the game-ready ~3.3k-tri cut (1.7 MB) — over the 95k-tri authoring
/// mesh (49.6 MB): fitting only needs the body's SHAPE, and the dense one costs 29× the load and
/// weight-transfer work for no fitting benefit. Falls back to the HQ Golem, then the canonical human
/// base, so any tree still bakes.
pub fn fitting_base() -> std::path::PathBuf {
    let characters = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../Alpha/content/characters");
    for candidate in ["GolemBase_Low", "GolemBase"] {
        let p = characters.join(candidate).join(format!("{candidate}.json"));
        if p.exists() {
            return p;
        }
    }
    crate::conform::default_reference()
}

/// A reasonable STARTING socket bone for a garment, inferred from its name — mirrors
/// `skin_outfit.py`'s `PIECE_SOCKET`, defaulting to the upper spine for an unrecognised torso
/// piece. Only seeds the editor's socket picker; the user then confirms or changes it.
pub fn garment_socket(name: &str) -> &'static str {
    let n = name.to_ascii_lowercase();
    if n.contains("pant") || n.contains("hem") || n.contains("skirt") || n.contains("belt") || n.contains("leg") {
        "pelvis"
    } else if n.contains("boot") || n.contains("shoe") || n.contains("foot") {
        "calf_l"
    } else if n.contains("glove") || n.contains("gauntlet") || n.contains("hand") || n.contains("bracer") {
        "lowerarm_l"
    } else if n.contains("hood") || n.contains("hat") || n.contains("helm") || n.contains("mask") {
        "head"
    } else {
        "spine_02"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conform::{conform_to_canonical, default_reference};
    use crate::fbx::{parse_fbx, RawBone, RawVertex};
    use crate::rig::rename_to_canonical;

    fn find_character() -> Option<std::path::PathBuf> {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../Alpha/content/source/PrismHumanBaseA");
        std::fs::read_dir(&dir).ok()?.filter_map(|e| e.ok().map(|e| e.path())).find(|p| {
            p.to_string_lossy().contains("Character_output") && p.extension().map(|e| e == "fbx").unwrap_or(false)
        })
    }

    /// A real static-PROP FBX (a weapon) — no skeleton. Skips the prop tests when the content tree
    /// is absent, the same guard the character tests use.
    fn find_prop() -> Option<std::path::PathBuf> {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../Alpha/content/source/PrismWeaps/MuseEpicSet");
        std::fs::read_dir(&dir)
            .ok()?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .find(|p| p.extension().map(|e| e == "fbx").unwrap_or(false))
    }

    /// A one-vertex stand-in, so the write paths can be exercised without a multi-megabyte parse.
    fn tiny_model() -> RawModel {
        RawModel {
            vertices: vec![RawVertex {
                p: [0.0, 0.0, 0.0],
                n: [0.0, 0.0, 1.0],
                uv: [0.5, 0.5],
                joints: [0; 4],
                weights: [0.0; 4],
            }],
            indices: vec![0],
            bones: Vec::new(),
        }
    }

    /// The PROP bake CARRIES ITS TEXTURES — the bug this closes. A prop used to land as a lone
    /// `.json` with the flat placeholder material while its maps stayed behind in the source folder,
    /// so the paperdoll drew it untextured. It now runs the SAME wiring the character import does
    /// (`pipeline::wire_textures`): the maps are copied beside the rig as `<AssetName>_<Map>.png` and
    /// the material points at those basenames.
    ///
    /// Hermetic — a synthetic TWO-piece folder, the shape a Meshy weapon set / outfit really arrives
    /// in, which also pins the two ways this can go quietly wrong: a piece must take ITS OWN maps and
    /// not a sibling's, and the packed `…_metallic_roughness` must not displace the dedicated
    /// `…_metallic`.
    #[test]
    fn write_prop_copies_the_source_maps_beside_the_rig() {
        let root = std::env::temp_dir().join("flicker_content_prop_textures");
        let (src, out_dir) = (root.join("source"), root.join("out"));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&src).unwrap();
        let fbx = src.join("Piece_A_texture.fbx");
        for (name, body) in [
            ("Piece_A_texture.fbx", "fbx"),
            ("Piece_A_texture.png", "A-base"),
            ("Piece_A_texture_metallic.png", "A-metal"),
            ("Piece_A_texture_roughness.png", "A-rough"),
            ("Piece_A_texture_normal.png", "A-normal"),
            ("Piece_A_texture_metallic_roughness.png", "A-packed"),
            ("Piece_A_texture_emission.png", "A-emit"),
            // The sibling piece in the same folder — its maps must NOT be picked up.
            ("Piece_B_texture.fbx", "fbx"),
            ("Piece_B_texture.png", "B-base"),
            ("Piece_B_texture_metallic.png", "B-metal"),
        ] {
            std::fs::write(src.join(name), body).unwrap();
        }

        let out = out_dir.join("PieceA.json");
        write_prop(&tiny_model(), &fbx, "PieceA", &out, &Fit::default()).expect("the prop writes");

        let rig: RigFile =
            serde_json::from_str(&std::fs::read_to_string(&out).expect("the rig was written")).unwrap();
        let m = rig.mesh.materials.first().expect("the prop has a material");
        assert_eq!(m.base_color, "PieceA_BaseColor.png", "albedo wired");
        assert_eq!(m.metalness, "PieceA_Metallic.png");
        assert_eq!(m.roughness, "PieceA_Roughness.png");
        assert_eq!(m.normal, "PieceA_Normal.png");
        assert_eq!(m.name, "PieceA", "named for the asset, as the character import names it");
        assert_eq!(m.slot, "PieceA");

        // The BYTES say which file actually landed: this piece's maps, and the dedicated metallic —
        // not the sibling's albedo, not the packed `metallic_roughness`.
        let beside = |n: &str| {
            std::fs::read_to_string(out_dir.join(n))
                .unwrap_or_else(|e| panic!("{n} must sit beside the rig: {e}"))
        };
        assert_eq!(beside("PieceA_BaseColor.png"), "A-base", "this piece's albedo, not the sibling's");
        assert_eq!(beside("PieceA_Metallic.png"), "A-metal", "the dedicated metallic, not the packed map");
        assert_eq!(beside("PieceA_Roughness.png"), "A-rough");
        assert_eq!(beside("PieceA_Normal.png"), "A-normal");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The GARMENT half of the same fix: a garment takes the BODY's skin weights but keeps its OWN
    /// material, so its source maps travel with it exactly as a prop's do. Needs the fitting body
    /// (`fitting_base`) to skin onto; skips cleanly without it.
    #[test]
    fn write_garment_copies_the_source_maps_beside_the_rig() {
        let body_path = fitting_base();
        let Ok(body_text) = std::fs::read_to_string(&body_path) else {
            eprintln!("skipping: no fitting body at {}", body_path.display());
            return;
        };
        let body: RigFile = serde_json::from_str(&body_text).expect("the fitting body parses");
        let socket = ["spine_02", "spine_01", "pelvis"]
            .into_iter()
            .find(|s| body.skeleton.bones.iter().any(|b| b.name == *s));
        let (Some(socket), false) = (socket, body.mesh.vertices.is_empty()) else {
            eprintln!("skipping: the fitting body has no torso socket / no mesh");
            return;
        };

        let root = std::env::temp_dir().join("flicker_content_garment_textures");
        let (src, out_dir) = (root.join("source"), root.join("out"));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&src).unwrap();
        let fbx = src.join("Top_Duster_texture.fbx");
        for (name, bytes) in [
            ("Top_Duster_texture.fbx", "fbx"),
            ("Top_Duster_texture.png", "duster-base"),
            ("Top_Duster_texture_roughness.png", "duster-rough"),
        ] {
            std::fs::write(src.join(name), bytes).unwrap();
        }

        let out = out_dir.join("Duster.json");
        let fit = Fit { socket: socket.to_string(), ..Default::default() };
        write_garment(&tiny_model(), &fbx, "Duster", &out, &fit).expect("the garment writes");

        let rig: RigFile =
            serde_json::from_str(&std::fs::read_to_string(&out).expect("the rig was written")).unwrap();
        let m = rig.mesh.materials.first().expect("the garment has a material");
        assert_eq!(m.base_color, "Duster_BaseColor.png");
        assert_eq!(m.roughness, "Duster_Roughness.png");
        assert_eq!(
            std::fs::read_to_string(out_dir.join("Duster_BaseColor.png")).expect("copied beside the rig"),
            "duster-base"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The same wiring against the REAL folder that showed the bug: the hair prop the user imported
    /// carries five Meshy maps, and its bake must reference + copy the four the material has slots
    /// for (the emission map has no renderer behind it, so it is deliberately left alone). Uses a
    /// stand-in mesh — what is under test is the folder→material wiring, not the 22 MB FBX parse
    /// (`bake_prop_has_export_prop_shape` covers that). Skips when the content tree is absent.
    #[test]
    fn write_prop_wires_the_real_hair_props_maps() {
        let src = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../Alpha/content/source/PrismHumanBaseHairColor");
        let Some(fbx) = std::fs::read_dir(&src).ok().and_then(|rd| {
            rd.filter_map(|e| e.ok().map(|e| e.path()))
                .find(|p| p.extension().map(|e| e == "fbx").unwrap_or(false))
        }) else {
            eprintln!("skipping: no PrismHumanBaseHairColor source");
            return;
        };
        let dir = std::env::temp_dir().join("flicker_content_hair_prop_textures");
        let _ = std::fs::remove_dir_all(&dir);
        let name = "PrismHumanBaseHairColor";
        let out = dir.join(format!("{name}.json"));
        write_prop(&tiny_model(), &fbx, name, &out, &Fit::default()).expect("the hair prop writes");

        let rig: RigFile =
            serde_json::from_str(&std::fs::read_to_string(&out).expect("the rig was written")).unwrap();
        let m = rig.mesh.materials.first().expect("the prop has a material");
        for (slot, got) in [
            ("BaseColor", &m.base_color),
            ("Metallic", &m.metalness),
            ("Roughness", &m.roughness),
            ("Normal", &m.normal),
        ] {
            let want = format!("{name}_{slot}.png");
            assert_eq!(got.as_str(), want, "{slot} referenced by the content-standard basename");
            let copied = dir.join(&want);
            assert!(copied.exists(), "{want} copied beside the rig");
            assert!(std::fs::metadata(&copied).unwrap().len() > 0, "{want} is not empty");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The PROP bake: a bone-less weapon FBX parses (proving the `parse_fbx` no-skeleton relaxation)
    /// and bakes to the `export_prop` shape — an EMPTY skeleton, the mesh verbatim with its inert
    /// zero skin, `retarget:false`, and crucially NO synthesized root / NO `+1` joint shift. The
    /// Python oracle `Dagger.json` has the same shape.
    #[test]
    fn bake_prop_has_export_prop_shape() {
        let Some(fbx) = find_prop() else {
            eprintln!("skipping: no PrismWeaps source");
            return;
        };
        // The relaxation in `parse_fbx`: a prop FBX with no skeleton parses instead of bailing.
        let model = parse_fbx(&fbx).expect("a bone-less prop FBX parses");
        assert!(model.bones.is_empty(), "a weapon carries no skeleton");
        assert!(!model.vertices.is_empty(), "but it has geometry");

        let rig = bake_prop(&model, "Dagger");
        assert!(rig.skeleton.bones.is_empty(), "a prop bakes with NO skeleton (no synthesized root)");
        assert!(!rig.retarget, "a rigid prop does not retarget");
        assert_eq!(rig.mesh.vertices.len(), model.vertices.len(), "every vertex carried through");
        assert_eq!(rig.mesh.indices.len(), rig.mesh.vertices.len(), "sequential-triple indices");
        assert_eq!(rig.mesh.submeshes.len(), 1);
        assert_eq!(rig.mesh.submeshes[0].count, rig.mesh.indices.len(), "one submesh spans the mesh");
        // The skin is inert and UNSHIFTED — there is no root to shift onto, so `[0;4]` stays `[0;4]`
        // (the character bake's `+1` shift would point these at a non-existent bone 1).
        assert!(
            rig.mesh.vertices.iter().all(|v| v.joints == [0, 0, 0, 0] && v.weights == [0.0; 4]),
            "an unrigged prop keeps its zero skin verbatim"
        );
        assert_eq!(rig.source.source_axis, "Z_up");
        assert_eq!(rig.source.source_unit, "cm");

        // Round-trips through the real deserializer the paperdoll loads with.
        let json = serde_json::to_string(&rig).unwrap();
        let back: RigFile = serde_json::from_str(&json).unwrap();
        assert!(back.skeleton.bones.is_empty());
        assert_eq!(back.mesh.vertices.len(), rig.mesh.vertices.len());

        // The Python oracle, when present, has the SAME shape — proving the target, not just
        // internal consistency.
        let oracle = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../Alpha/content/characters/musefit/Dagger.json");
        if let Ok(text) = std::fs::read_to_string(&oracle) {
            let d: RigFile = serde_json::from_str(&text).expect("oracle Dagger.json parses");
            assert!(d.skeleton.bones.is_empty(), "oracle prop has no skeleton");
            assert!(!d.retarget, "oracle prop is retarget:false");
        }
    }

    /// A synthetic 2-bone body + a one-vertex garment pins the garment bake's two load-bearing
    /// steps without any real asset (so it always runs): the vertex is placed through the SOCKET's
    /// rest frame, and it takes the NEAREST body vertex's skin — the chest-bound one, not the
    /// root-bound one at the origin.
    #[test]
    fn garment_skins_onto_the_nearest_body_vertex() {
        // chest bone rest world = translate +100 z, so its inverse_bind translates −100 z.
        let ib_chest = Mat4::from_translation(Vec3::new(0.0, 0.0, -100.0)).to_cols_array();
        let body = RigFile {
            format: "flicker.rig".to_string(),
            version: 1,
            retarget: true,
            source: Source::default(),
            skeleton: Skeleton {
                bones: vec![
                    BoneRaw { name: "root".into(), parent: -1, local: IDENTITY16, inverse_bind: IDENTITY16 },
                    BoneRaw { name: "chest".into(), parent: 0, local: IDENTITY16, inverse_bind: ib_chest },
                ],
            },
            mesh: Mesh {
                vertices: vec![
                    // chest-bound, AT the chest rest position — the nearest to the baked garment vert.
                    Vertex { p: [0.0, 0.0, 100.0], n: [0.0, 0.0, 1.0], uv: [0.0, 0.0], joints: [1, 0, 0, 0], weights: [1.0, 0.0, 0.0, 0.0] },
                    // root-bound, at the origin — farther away, must NOT be chosen.
                    Vertex { p: [0.0, 0.0, 0.0], n: [0.0, 0.0, 1.0], uv: [0.0, 0.0], joints: [0, 0, 0, 0], weights: [1.0, 0.0, 0.0, 0.0] },
                ],
                indices: vec![0, 1, 0],
                submeshes: vec![Submesh { material: 0, start: 0, count: 3 }],
                materials: vec![Material { name: "m".into(), slot: "m".into(), ..Default::default() }],
                ..Default::default()
            },
            clips: Vec::new(),
            attach: Default::default(),
            collision: Default::default(),
        };
        let garment = RawModel {
            vertices: vec![RawVertex { p: [0.0, 0.0, 0.0], n: [0.0, 0.0, 1.0], uv: [0.3, 0.4], joints: [0; 4], weights: [0.0; 4] }],
            indices: vec![0],
            bones: Vec::new(),
        };
        let fit = Attach {
            socket: "chest".into(),
            offset: [0.0; 3],
            rotate: [0.0, 0.0, 0.0, 1.0],
            scale: [1.0; 3],
            uniform: 1.0,
        };
        let rig = bake_garment(&garment, "TestGarment", &body, "chest", &fit).expect("garment bakes");

        // Carries the body's FULL skeleton (so load_outfit's by-name remap is the identity).
        assert_eq!(rig.skeleton.bones.len(), 2);
        assert_eq!(rig.skeleton.bones[1].name, "chest");
        assert!(!rig.retarget, "a garment does not retarget");
        assert_eq!(rig.source.applied_transform, "baked-fit");

        let v = &rig.mesh.vertices[0];
        assert!(
            (Vec3::from(v.p) - Vec3::new(0.0, 0.0, 100.0)).length() < 1e-3,
            "placed at the socket rest frame, got {:?}",
            v.p
        );
        assert_eq!(v.joints, [1, 0, 0, 0], "took the chest-bound body vertex's joints");
        assert_eq!(v.weights, [1.0, 0.0, 0.0, 0.0]);
        assert_eq!(v.uv, [0.3, 0.4], "the garment keeps its own UV");
    }

    /// The spatial grid must be EXACT — it has to return what a brute-force argmin would, or a
    /// garment silently skins to the wrong side of a fold (exactly why `skin_outfit.py` refused a
    /// capped search). Checked against a brute-force oracle over a scattered, human-scale body.
    #[test]
    fn vertex_grid_matches_brute_force() {
        let verts: Vec<Vertex> = (0..600u32)
            .map(|i| {
                let f = i as f32;
                Vertex {
                    p: [(f * 7.3) % 60.0 - 30.0, (f * 3.1) % 90.0 - 45.0, (f * 11.7) % 180.0],
                    n: [0.0, 0.0, 1.0],
                    uv: [0.0, 0.0],
                    joints: [i % 60, 0, 0, 0],
                    weights: [1.0, 0.0, 0.0, 0.0],
                }
            })
            .collect();
        let grid = VertexGrid::build(&verts);
        let brute = |p: Vec3| {
            let (mut b, mut bd) = (0usize, f32::INFINITY);
            for (i, v) in verts.iter().enumerate() {
                let d = (Vec3::from(v.p) - p).length_squared();
                if d < bd {
                    bd = d;
                    b = i;
                }
            }
            b
        };
        for k in 0..250u32 {
            let f = k as f32;
            let p = Vec3::new(
                (f * 5.7) % 70.0 - 35.0,
                (f * 2.3) % 100.0 - 50.0,
                (f * 13.1) % 200.0 - 10.0,
            );
            let (g, b) = (grid.nearest(p, &verts), brute(p));
            // Ties are legitimate, so compare the DISTANCE — what the transfer actually depends on.
            let dg = (Vec3::from(verts[g].p) - p).length_squared();
            let db = (Vec3::from(verts[b].p) - p).length_squared();
            assert!((dg - db).abs() < 1e-4, "grid picked a farther vertex at {p}: {dg} vs {db}");
        }
    }

    /// END-TO-END on REAL assets: parse a raw Muse fit FBX, skin it onto a baked body, and confirm
    /// a valid outfit overlay — the body's full skeleton, every vertex re-skinned to real body
    /// bones with normalised weights. `#[ignore]` (exact nearest-vertex over ~12k body × ~44k
    /// garment verts is heavy in debug); run explicitly:
    ///   cargo test -p flicker-content -- --ignored bake_garment_real
    #[test]
    #[ignore]
    fn bake_garment_real_skins_a_muse_fit_onto_the_body() {
        // The FITTING body (the clay Golem when present) — the same one `write_garment` skins onto.
        let body_path = fitting_base();
        let Ok(body_text) = std::fs::read_to_string(&body_path) else {
            eprintln!("skipping: no baked body at {}", body_path.display());
            return;
        };
        let body: RigFile = serde_json::from_str(&body_text).expect("baked body parses");
        if body.mesh.vertices.is_empty() || body.skeleton.bones.is_empty() {
            eprintln!("skipping: body carries no skinned mesh");
            return;
        }
        let fit_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../Alpha/content/source/PrismFits/Muse001");
        let Some(fbx) = std::fs::read_dir(&fit_dir).ok().and_then(|rd| {
            rd.filter_map(|e| e.ok().map(|e| e.path()))
                .find(|p| p.extension().map(|e| e == "fbx").unwrap_or(false))
        }) else {
            eprintln!("skipping: no Muse fit source");
            return;
        };
        let garment = parse_fbx(&fbx).expect("the raw garment FBX parses");
        assert!(garment.bones.is_empty(), "a raw Meshy garment carries no skeleton");

        let socket = ["spine_02", "spine_01", "pelvis"]
            .into_iter()
            .find(|s| body.skeleton.bones.iter().any(|b| b.name == *s))
            .expect("the body has a torso socket");
        let fit = Attach { socket: socket.into(), offset: [0.0; 3], rotate: [0.0, 0.0, 0.0, 1.0], scale: [1.0; 3], uniform: 1.0 };
        let rig = bake_garment(&garment, "Muse001", &body, socket, &fit).expect("garment bakes");

        assert_eq!(rig.skeleton.bones.len(), body.skeleton.bones.len(), "carries the body's full skeleton");
        assert_eq!(rig.mesh.vertices.len(), garment.vertices.len(), "every garment vertex skinned");
        assert_eq!(rig.source.applied_transform, "baked-fit");
        let nb = body.skeleton.bones.len() as u32;
        for v in &rig.mesh.vertices {
            assert!(v.joints.iter().all(|&j| j < nb), "joints index the body skeleton");
            let w: f32 = v.weights.iter().sum();
            assert!((w - 1.0).abs() < 0.05 || w == 0.0, "transferred weights normalised, got {w}");
            assert!(Vec3::from(v.p).is_finite() && Vec3::from(v.n).is_finite());
        }
        // Round-trips through the real deserializer the paperdoll loads with.
        let json = serde_json::to_string(&rig).unwrap();
        let _back: RigFile = serde_json::from_str(&json).unwrap();
        eprintln!("baked {} garment verts onto {} body bones", rig.mesh.vertices.len(), nb);
    }

    /// The bake of the conformed female base yields the oracle's shape: 66 bones with a synthesized
    /// identity `root` at 0, `pelvis` parented to it, the face group under `head`, and the mesh
    /// carried through with joints shifted for the root.
    #[test]
    fn bake_of_conform_has_oracle_shape() {
        let (Some(fbx), reference) = (find_character(), default_reference()) else {
            eprintln!("skipping: no source / reference");
            return;
        };
        if !reference.exists() {
            return;
        }
        let mut model = parse_fbx(&fbx).unwrap();
        rename_to_canonical(&mut model);
        conform_to_canonical(&mut model, &reference).unwrap();
        let rig = bake_rig(&model, "PrismHumanBaseA");

        assert_eq!(rig.skeleton.bones.len(), 66, "65 conformed + synthesized root");
        assert_eq!(rig.skeleton.bones[0].name, "root");
        assert_eq!(rig.skeleton.bones[0].parent, -1);
        assert_eq!(rig.skeleton.bones[0].local, IDENTITY16, "root is identity");
        let by_name = |n: &str| rig.skeleton.bones.iter().position(|b| b.name == n);
        let (pelvis, head) = (by_name("pelvis").unwrap(), by_name("head").unwrap());
        assert_eq!(rig.skeleton.bones[pelvis].parent, 0, "pelvis parents to root");
        for n in ["jaw", "eye_l", "eye_r"] {
            assert_eq!(rig.skeleton.bones[by_name(n).unwrap()].parent as usize, head, "{n} under head");
        }
        // Every parent index is valid, and the tree is topological (parent precedes child).
        for (i, b) in rig.skeleton.bones.iter().enumerate() {
            assert!(b.parent == -1 || (b.parent >= 0 && (b.parent as usize) < i), "bone {i} '{}' parent {} precedes it", b.name, b.parent);
        }
        assert_eq!(rig.mesh.vertices.len(), model.vertices.len());
        assert_eq!(rig.mesh.submeshes.len(), 1);
        assert_eq!(rig.mesh.submeshes[0].count, rig.mesh.indices.len());
        // Joints are valid bone indices (shifted into 1..=65, weighted ones real).
        let nb = rig.skeleton.bones.len() as u32;
        assert!(rig.mesh.vertices.iter().all(|v| v.joints.iter().all(|&j| j < nb)));
    }

    /// END-TO-END real pipeline: parse → rename → conform → bake → WRITE the flicker.rig → reload it
    /// through the real `flicker_skeletal::format::load_dir`. Confirms the in-app bake produces a file
    /// the engine loader actually accepts (66 bones, mesh present), and reports its size. `#[ignore]`d
    /// because it writes a large JSON; run explicitly:
    ///   `cargo test -p flicker-content -- --ignored bakes_and_reloads`
    #[test]
    #[ignore]
    fn bakes_and_reloads_through_the_engine_loader() {
        let (Some(fbx), reference) = (find_character(), default_reference()) else { return };
        if !reference.exists() {
            return;
        }
        let mut model = parse_fbx(&fbx).unwrap();
        rename_to_canonical(&mut model);
        conform_to_canonical(&mut model, &reference).unwrap();

        let dir = std::env::temp_dir().join("flicker_content_bake_e2e");
        let _ = std::fs::create_dir_all(&dir);
        let out = dir.join("PrismHumanBaseA.json");
        write_rig(&model, &fbx, "PrismHumanBaseA", &out).unwrap();
        let bytes = std::fs::metadata(&out).unwrap().len();
        eprintln!("wrote {} ({:.1} MB)", out.display(), bytes as f64 / 1.0e6);

        let loaded = flicker_skeletal::format::load_dir(&dir).expect("engine loader accepts the bake");
        eprintln!("engine loaded {} bones, {} verts", loaded.bones.len(), loaded.mesh.vertices.len());
        assert_eq!(loaded.bones.len(), 66, "engine sees 66 bones");
        assert_eq!(loaded.mesh.vertices.len(), model.vertices.len(), "mesh carried through");
        assert!(loaded.bones.iter().any(|b| b.name == "root"), "root present");
        // `write_rig` is the EDITOR's commit path (the CLI `import_folder` wires its own). It must
        // bring the source's maps along, or a character committed from the bench ships UNTEXTURED —
        // exactly the gap props carried until they were routed through the same call. Guarded here
        // because that gap was rediscovered once already.
        assert_eq!(
            loaded.mesh.materials.first().map(|m| m.base_color.as_str()),
            Some("PrismHumanBaseA_BaseColor.png"),
            "the committed character's material must reference its copied albedo"
        );
        assert!(
            dir.join("PrismHumanBaseA_BaseColor.png").exists(),
            "and the map itself is written beside the rig"
        );
        // Provenance too, not only the material: `source.textures` is the manifest a validator reads
        // to answer "does this asset carry its maps?" — it must list what was actually copied.
        assert!(
            loaded.source.textures.iter().any(|t| t == "PrismHumanBaseA_BaseColor.png"),
            "the copied maps are recorded in the rig's source provenance, got {:?}",
            loaded.source.textures
        );
        // Whole dir: the bake now leaves ~57 MB of copied maps beside the rig, not just the JSON.
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The canonical types serialize and the JSON round-trips through the real `flicker-skeletal`
    /// deserializer — proving the additive `Serialize` derive and the root synthesis on a tiny model.
    #[test]
    fn serialized_rig_round_trips_through_the_loader() {
        let model = RawModel {
            vertices: vec![RawVertex {
                p: [1.0, 2.0, 3.0],
                n: [0.0, 0.0, 1.0],
                uv: [0.25, 0.5],
                joints: [0, 0, 0, 0],
                weights: [1.0, 0.0, 0.0, 0.0],
            }],
            indices: vec![0],
            bones: vec![
                RawBone { name: "pelvis".into(), parent: -1, translation: [0.0, 0.0, 95.0], rotation: [0.0, 0.0, 0.0, 1.0], scale: [1.0; 3], inverse_bind: IDENTITY16 },
                RawBone { name: "spine_01".into(), parent: 0, translation: [0.0, 0.0, 10.0], rotation: [0.0, 0.0, 0.0, 1.0], scale: [1.0; 3], inverse_bind: IDENTITY16 },
            ],
        };
        let rig = bake_rig(&model, "tiny");
        let json = serde_json::to_string(&rig).unwrap();
        let back: RigFile = serde_json::from_str(&json).unwrap();
        assert_eq!(back.skeleton.bones.len(), 3, "root + pelvis + spine_01");
        assert_eq!(back.skeleton.bones[0].name, "root");
        assert_eq!(back.skeleton.bones[1].name, "pelvis");
        assert_eq!(back.skeleton.bones[1].parent, 0, "pelvis re-parented onto root");
        assert_eq!(back.skeleton.bones[2].parent, 1, "spine_01 parent shifted +1");
        assert_eq!(back.mesh.vertices[0].joints, [1, 1, 1, 1], "joints shifted for the root");
        assert!(back.retarget);
        assert_eq!(back.source.source_axis, "Z_up");
    }
}
