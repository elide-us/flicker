//! FBX → intermediate model — the parse stage (WS-F slice 2), in-app via the `ufbx` library.
//!
//! Reads a raw FBX into a convention-NEUTRAL [`RawModel`]: one combined triangulated mesh (per
//! triangle-corner vertices, matching flicker.rig's non-deduped convention) + the skeleton (bones
//! with local TRS + parent) + per-vertex 4-influence skin. Axis/unit normalisation and the
//! canonical-rig conform happen in the later slices; this stage only faithfully extracts what the
//! FBX contains. No `unsafe` here — the ufbx Rust wrapper is safe.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use anyhow::{bail, Context, Result};
use glam::{Mat4, Vec3};

/// One parsed vertex (per triangle-corner; the mesh is emitted non-deduped with sequential indices,
/// the flicker.rig convention). Positions/normals are in the FBX's native space.
#[derive(Debug, Clone, Copy)]
pub struct RawVertex {
    pub p: [f32; 3],
    pub n: [f32; 3],
    pub uv: [f32; 2],
    /// Bone indices into [`RawModel::bones`], zero-padded.
    pub joints: [u32; 4],
    /// 4-influence weights, top-4, normalised to sum 1 (0 for an unskinned vertex).
    pub weights: [f32; 4],
}

/// One parsed skeleton bone.
#[derive(Debug, Clone)]
pub struct RawBone {
    pub name: String,
    /// Index into [`RawModel::bones`], or `-1` for a root bone.
    pub parent: i32,
    /// Local (bone→parent) transform, convention-neutral (the FBX node's local transform).
    pub translation: [f32; 3],
    pub rotation: [f32; 4], // quaternion xyzw
    pub scale: [f32; 3],
    /// Inverse bind = inverse of the bone's rest WORLD frame, column-major (`Mat4::to_cols_array`).
    /// Vertices are emitted in world space (see [`parse_fbx`]), so this is `world.inverse()`, which
    /// makes rest-pose skinning the identity. The conform slice rebuilds it from the reoriented frame.
    pub inverse_bind: [f32; 16],
}

/// The raw parsed model — one mesh + the skeleton, in the FBX's native space.
#[derive(Debug, Clone)]
pub struct RawModel {
    pub vertices: Vec<RawVertex>,
    pub indices: Vec<u32>,
    pub bones: Vec<RawBone>,
}

/// A quarter-turn (90°) about a world axis, as an EXACT integer matrix.
///
/// Built by axis permutation rather than `sin`/`cos`, so applying four turns returns the model
/// bit-for-bit to where it started — an orientation control the user nudges repeatedly must not
/// accumulate float drift into the baked rig.
pub fn quarter_turn(axis: usize) -> Mat4 {
    let (x, y, z) = match axis {
        0 => (Vec3::X, Vec3::Z, Vec3::NEG_Y), // about X: y→z, z→−y
        1 => (Vec3::NEG_Z, Vec3::Y, Vec3::X), // about Y: z→x, x→−z
        _ => (Vec3::Y, Vec3::NEG_X, Vec3::Z), // about Z: x→y, y→−x
    };
    Mat4::from_cols(
        x.extend(0.0),
        y.extend(0.0),
        z.extend(0.0),
        Vec3::ZERO.extend(1.0),
    )
}

/// Rotate a whole parsed asset into a different **ground reckoning** — every vertex and the ROOT
/// bone frames — so a source authored on its side (or with a lying up-axis its FBX header did not
/// declare) is stood up BEFORE conform and bake. Children follow their root through the hierarchy,
/// so only root locals move; `inverse_bind` is the inverse of each bone's rest WORLD, so it takes
/// the inverse rotation on the right and rest-pose skinning stays the identity.
pub fn apply_orientation(model: &mut RawModel, r: Mat4) {
    let rot3 = glam::Mat3::from_mat4(r);
    let r_inv = r.inverse();
    for v in &mut model.vertices {
        v.p = r.transform_point3(Vec3::from(v.p)).to_array();
        v.n = (rot3 * Vec3::from(v.n)).normalize_or_zero().to_array();
    }
    for b in &mut model.bones {
        if b.parent < 0 {
            let local = Mat4::from_scale_rotation_translation(
                Vec3::from(b.scale),
                glam::Quat::from_array(b.rotation),
                Vec3::from(b.translation),
            );
            let (s, q, t) = (r * local).to_scale_rotation_translation();
            b.scale = s.to_array();
            b.rotation = q.to_array();
            b.translation = t.to_array();
        }
        let ib = Mat4::from_cols_array(&b.inverse_bind);
        b.inverse_bind = (ib * r_inv).to_cols_array();
    }
}

/// Parse an FBX file into a [`RawModel`], normalised to the engine's space: **Z-up, centimetres,
/// right-handed** (Meshy authors Y-up metres). ufbx applies the axis/unit conversion, and we bake
/// each mesh's `geometry_to_world` into the vertices so positions land in world/armature space —
/// exactly what the `flicker.rig` oracle stores, and what makes bone rest frames and vertices share
/// one space (so `inverse_bind = world.inverse()` gives rest-skin identity).
pub fn parse_fbx(path: &Path) -> Result<RawModel> {
    let path_str = path.to_str().context("FBX path is not valid UTF-8")?;
    let opts = ufbx::LoadOpts {
        target_axes: ufbx::CoordinateAxes::right_handed_z_up(),
        target_unit_meters: 0.01,
        space_conversion: ufbx::SpaceConversion::AdjustTransforms,
        ..Default::default()
    };
    let scene = ufbx::load_file(path_str, opts)
        .map_err(|e| anyhow::anyhow!("ufbx failed to load {}: {:?}", path.display(), e))?;

    // A bone-less mesh is NOT an error: a static PROP (a weapon, an accessory, a raw garment
    // before it is skinned onto a body) IS exactly a mesh with no skeleton. It returns
    // `bones: []`, every vertex carries the inert `([0;4], [0.0;4])` skin `skin_table` yields
    // with no clusters, and the prop/garment bake handles that (empty `skeleton.bones`, no root
    // synthesis, no +1 joint shift). Downstream classification reads `bones.is_empty()` → Prop.
    // Only a mesh with NO GEOMETRY (the `vertices.is_empty()` guard below) is a real failure.
    let bones = parse_skeleton(&scene);
    let bone_index: HashMap<u32, usize> = element_index(&scene);

    // Iterate nodes (not `scene.meshes`) so each mesh's `geometry_to_world` is in hand — that matrix
    // carries the axis/unit conversion, taking geometry-local verts into world/armature space.
    let mut vertices = Vec::new();
    for node in &scene.nodes {
        let Some(mesh) = node.mesh.as_ref() else {
            continue;
        };
        let g2w = &node.geometry_to_world;
        let skin = skin_table(mesh, &bone_index);
        let mut tri = vec![0u32; mesh.max_face_triangles * 3];
        for face in &mesh.faces {
            let ntri = mesh.triangulate_face(&mut tri, *face) as usize;
            for &corner in &tri[..ntri * 3] {
                let ci = corner as usize;
                let p = ufbx::transform_position(g2w, mesh.vertex_position[ci]);
                let n = if mesh.vertex_normal.exists {
                    let d = ufbx::transform_direction(g2w, mesh.vertex_normal[ci]);
                    let len = (d.x * d.x + d.y * d.y + d.z * d.z).sqrt();
                    if len > 1e-9 {
                        ufbx::Vec3 {
                            x: d.x / len,
                            y: d.y / len,
                            z: d.z / len,
                        }
                    } else {
                        d
                    }
                } else {
                    ufbx::Vec3::default()
                };
                let uv = if mesh.vertex_uv.exists {
                    // flicker.rig stores TOP-origin UVs (`v → 1−v`) — the exporter's contract
                    // (io_scene_flicker_rig.py). FBX/ufbx UVs are bottom-origin, so flip V here or
                    // the texture atlas reads scrambled.
                    let uv = mesh.vertex_uv[ci];
                    ufbx::Vec2 {
                        x: uv.x,
                        y: 1.0 - uv.y,
                    }
                } else {
                    ufbx::Vec2::default()
                };
                let uvtx = mesh.vertex_indices[ci] as usize;
                let (joints, weights) = skin.get(uvtx).copied().unwrap_or(([0; 4], [0.0; 4]));
                vertices.push(RawVertex {
                    p: [p.x as f32, p.y as f32, p.z as f32],
                    n: [n.x as f32, n.y as f32, n.z as f32],
                    uv: [uv.x as f32, uv.y as f32],
                    joints,
                    weights,
                });
            }
        }
    }
    if vertices.is_empty() {
        bail!("no mesh geometry found in {}", path.display());
    }
    let indices = (0..vertices.len() as u32).collect();
    Ok(RawModel {
        vertices,
        indices,
        bones,
    })
}

/// The skeleton = every node that is a bone (has a Bone attribute) OR is bound by a skin cluster,
/// parent-indexed. A bone whose parent isn't itself a bone becomes a root (`-1`) — the canonical-rig
/// slice restructures the hierarchy anyway.
///
/// Each bone's rest frame is read from ufbx's `node_to_world` (already Z-up/cm), and its local TRS is
/// derived relative to its *bone*-parent (`parent_world.inverse() * world`). Reading world frames —
/// rather than `local_transform` — is what makes this robust to the intermediate Armature/root node
/// the bone set excludes: any axis/unit conversion ufbx placed on that node is still captured here.
fn parse_skeleton(scene: &ufbx::Scene) -> Vec<RawBone> {
    let mut bound: HashSet<u32> = HashSet::new();
    for cluster in &scene.skin_clusters {
        if let Some(bn) = cluster.bone_node.as_ref() {
            bound.insert(bn.element.element_id);
        }
    }
    let bone_nodes: Vec<&ufbx::Node> = (&scene.nodes)
        .into_iter()
        .filter(|n| n.bone.is_some() || bound.contains(&n.element.element_id))
        .collect();

    let pos: HashMap<u32, usize> = bone_nodes
        .iter()
        .enumerate()
        .map(|(i, n)| (n.element.element_id, i))
        .collect();

    // World rest frame per bone (glam, Z-up/cm).
    let world: Vec<Mat4> = bone_nodes.iter().map(|n| mat4(&n.node_to_world)).collect();
    let parents: Vec<i32> = bone_nodes
        .iter()
        .map(|n| {
            n.parent
                .as_ref()
                .and_then(|p| pos.get(&p.element.element_id).copied())
                .map(|i| i as i32)
                .unwrap_or(-1)
        })
        .collect();

    bone_nodes
        .iter()
        .enumerate()
        .map(|(i, n)| {
            let local = if parents[i] < 0 {
                world[i]
            } else {
                world[parents[i] as usize].inverse() * world[i]
            };
            let (s, r, t) = local.to_scale_rotation_translation();
            RawBone {
                name: n.element.name.as_ref().to_string(),
                parent: parents[i],
                translation: t.to_array(),
                rotation: r.to_array(),
                scale: s.to_array(),
                inverse_bind: world[i].inverse().to_cols_array(),
            }
        })
        .collect()
}

/// A ufbx 3×4 column-major affine `Matrix` → glam `Mat4` (column-vector).
fn mat4(m: &ufbx::Matrix) -> Mat4 {
    Mat4::from_cols(
        Vec3::new(m.m00 as f32, m.m10 as f32, m.m20 as f32).extend(0.0),
        Vec3::new(m.m01 as f32, m.m11 as f32, m.m21 as f32).extend(0.0),
        Vec3::new(m.m02 as f32, m.m12 as f32, m.m22 as f32).extend(0.0),
        Vec3::new(m.m03 as f32, m.m13 as f32, m.m23 as f32).extend(1.0),
    )
}

/// Element-id → bone index, over the same bone set `parse_skeleton` builds (so skin weights resolve
/// to the same indices).
fn element_index(scene: &ufbx::Scene) -> HashMap<u32, usize> {
    let mut bound: HashSet<u32> = HashSet::new();
    for cluster in &scene.skin_clusters {
        if let Some(bn) = cluster.bone_node.as_ref() {
            bound.insert(bn.element.element_id);
        }
    }
    (&scene.nodes)
        .into_iter()
        .filter(|n| n.bone.is_some() || bound.contains(&n.element.element_id))
        .enumerate()
        .map(|(i, n)| (n.element.element_id, i))
        .collect()
}

/// Per-unique-vertex `(joints[4], weights[4])` for a mesh's first skin deformer (empty if the mesh
/// is unskinned). Top-4 influences by weight, normalised to sum 1.
fn skin_table(mesh: &ufbx::Mesh, bone_index: &HashMap<u32, usize>) -> Vec<([u32; 4], [f32; 4])> {
    let Some(def) = (&mesh.skin_deformers).into_iter().next() else {
        return Vec::new();
    };
    let weights_all = def.weights.as_ref();
    let mut out = vec![([0u32; 4], [0.0f32; 4]); mesh.num_vertices];
    for (v, slot) in out.iter_mut().enumerate() {
        let sv = def.vertices[v];
        let begin = sv.weight_begin as usize;
        let count = sv.num_weights as usize;
        let mut influences: Vec<(u32, f32)> = Vec::with_capacity(count);
        for w in &weights_all[begin..begin + count] {
            let cluster = &def.clusters[w.cluster_index as usize];
            if let Some(bn) = cluster.bone_node.as_ref() {
                if let Some(&bi) = bone_index.get(&bn.element.element_id) {
                    influences.push((bi as u32, w.weight as f32));
                }
            }
        }
        influences.sort_by(|a, b| b.1.total_cmp(&a.1));
        influences.truncate(4);
        let sum: f32 = influences.iter().map(|(_, w)| *w).sum();
        let (joints, weights) = slot;
        for (k, (bi, w)) in influences.iter().enumerate() {
            joints[k] = *bi;
            weights[k] = if sum > 0.0 { *w / sum } else { 0.0 };
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Roster inventory: scan the six PrismRaces bases, confirm the scanner picks exactly one riggable
    /// mesh per folder (the anim FBX is NOT mistaken for a rig), and parse each to report bone/poly
    /// counts. `#[ignore]`d (reads ~200 MB of FBX); run with
    ///   `cargo test -p flicker-content -- --ignored inventory_prismraces --nocapture`
    #[test]
    #[ignore]
    fn inventory_prismraces_roster() {
        use crate::scan::{scan_folder, Kind};
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../content/source/PrismRaces");
        if !root.exists() {
            eprintln!("skipping: {} not present", root.display());
            return;
        }
        let mut subs: Vec<_> = std::fs::read_dir(&root)
            .unwrap()
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.is_dir())
            .collect();
        subs.sort();
        eprintln!("base / bones / tris / verts / anim / tex / mesh-fbx");
        for sub in &subs {
            let s = scan_folder(sub).unwrap();
            let name = sub.file_name().unwrap().to_string_lossy().to_string();
            let tex = s.of_kind(Kind::Texture).count();
            let anim = s.of_kind(Kind::AnimFbx).count();
            match s.sole_riggable() {
                Some(rig) => {
                    let m = parse_fbx(&rig.path).unwrap();
                    eprintln!(
                        "{:<16} {:>6} {:>7} {:>9} {:>5} {:>4}  {}",
                        name,
                        m.bones.len(),
                        m.indices.len() / 3,
                        m.vertices.len(),
                        anim,
                        tex,
                        rig.path.file_name().unwrap().to_string_lossy()
                    );
                }
                None => eprintln!(
                    "{name:<16} !! riggable={} (needs disambiguation)",
                    s.riggable.len()
                ),
            }
        }
        assert_eq!(subs.len(), 6, "six race bases");
    }

    /// DIAGNOSTIC dump: HumanBaseA_Low's RAW Meshy skeleton — names, parents, world positions — to
    /// see the head/jaw/hip/foot arrangement Aaron described (straight-up head bone, forward jaw,
    /// Y-hip, flat vs pointed foot). `#[ignore]`d. Run:
    ///   `cargo test -p flicker-content -- --ignored dump_meshy_skeleton --nocapture`
    #[test]
    #[ignore]
    fn dump_meshy_skeleton() {
        use glam::{Mat4 as M, Quat as Q, Vec3 as V};
        let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../content/source/PrismRaces/HumanBaseA_Low");
        let Some(fbx) = dir.exists().then_some(()).and_then(|_| {
            std::fs::read_dir(&dir)
                .ok()?
                .filter_map(|e| e.ok().map(|e| e.path()))
                .find(|p| {
                    p.to_string_lossy().contains("Character_output")
                        && p.extension().map(|e| e == "fbx").unwrap_or(false)
                })
        }) else {
            eprintln!("skipping: no HumanBaseA_Low");
            return;
        };
        let model = parse_fbx(&fbx).unwrap();
        // world positions via FK
        let locals: Vec<M> = model
            .bones
            .iter()
            .map(|b| {
                M::from_scale_rotation_translation(
                    V::from(b.scale),
                    Q::from_array(b.rotation),
                    V::from(b.translation),
                )
            })
            .collect();
        let mut world = vec![M::IDENTITY; model.bones.len()];
        for i in 0..model.bones.len() {
            let p = model.bones[i].parent;
            world[i] = if p < 0 {
                locals[i]
            } else {
                world[p as usize] * locals[i]
            };
        }
        eprintln!(
            "HumanBaseA_Low RAW Meshy skeleton ({} bones):",
            model.bones.len()
        );
        for (i, b) in model.bones.iter().enumerate() {
            let pn = if b.parent < 0 {
                "-".into()
            } else {
                model.bones[b.parent as usize].name.clone()
            };
            let w = world[i].w_axis;
            eprintln!(
                "  {i:2} {:22} parent={:14} world=[{:6.1} {:6.1} {:6.1}]",
                b.name, pn, w.x, w.y, w.z
            );
        }
    }

    /// Guarded real-data parse: the actual female base `Character_output.fbx` yields a skinned biped
    /// — a skeleton, thousands of triangulated vertices, valid joint indices, and normalised weights.
    /// Skips when the source content isn't present.
    #[test]
    fn parses_the_real_female_base_character() {
        let dir =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../content/source/PrismHumanBaseA");
        if !dir.exists() {
            eprintln!("skipping: {} not present", dir.display());
            return;
        }
        let fbx = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok().map(|e| e.path()))
            .find(|p| {
                p.to_string_lossy().contains("Character_output")
                    && p.extension().map(|e| e == "fbx").unwrap_or(false)
            });
        let Some(fbx) = fbx else {
            eprintln!("skipping: no Character_output.fbx in {}", dir.display());
            return;
        };

        let model = parse_fbx(&fbx).expect("the character FBX parses");
        eprintln!(
            "parsed {}: {} bones, {} verts, {} tris",
            fbx.file_name().unwrap().to_string_lossy(),
            model.bones.len(),
            model.vertices.len(),
            model.indices.len() / 3
        );

        assert!(
            model.bones.len() >= 10,
            "a biped has a real skeleton, got {}",
            model.bones.len()
        );
        assert!(
            model.vertices.len() >= 1000,
            "a body mesh has geometry, got {}",
            model.vertices.len()
        );
        assert_eq!(
            model.indices.len(),
            model.vertices.len(),
            "non-deduped: one index per vertex"
        );
        assert_eq!(model.indices.len() % 3, 0, "triangulated");

        let nb = model.bones.len() as u32;
        assert!(
            model
                .vertices
                .iter()
                .all(|v| v.joints.iter().all(|&j| j < nb)),
            "every joint index is a valid bone"
        );
        // Parents are valid indices or -1.
        assert!(model
            .bones
            .iter()
            .all(|b| b.parent == -1 || (b.parent >= 0 && (b.parent as usize) < model.bones.len())));
        // At least one root, and most skinned verts sum to ~1.
        assert!(
            model.bones.iter().any(|b| b.parent == -1),
            "there is a root bone"
        );
        let skinned = model
            .vertices
            .iter()
            .filter(|v| v.weights.iter().sum::<f32>() > 0.5)
            .count();
        assert!(
            skinned >= model.vertices.len() / 2,
            "most vertices are skinned ({skinned})"
        );

        // Normalised to Z-up centimetres, world space: a ~170 cm biped stands on z≈0, is far taller
        // (z) than wide (x) or deep (y). If the axis/unit conversion were missing this would be y-up
        // metres and the ranges would be wrong. (Oracle: x:[-40.7,40.7] y:[-13.9,13.9] z:[0,170].)
        let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
        for v in &model.vertices {
            for k in 0..3 {
                lo[k] = lo[k].min(v.p[k]);
                hi[k] = hi[k].max(v.p[k]);
            }
        }
        eprintln!(
            "world verts cm: x:[{:.1},{:.1}] y:[{:.1},{:.1}] z:[{:.1},{:.1}]",
            lo[0], hi[0], lo[1], hi[1], lo[2], hi[2]
        );
        let (span_x, span_y, span_z) = (hi[0] - lo[0], hi[1] - lo[1], hi[2] - lo[2]);
        assert!(
            (150.0..200.0).contains(&hi[2]),
            "standing height ~170 cm in +z, got {}",
            hi[2]
        );
        assert!(lo[2] > -5.0 && lo[2] < 20.0, "feet near z=0, got {}", lo[2]);
        assert!(span_z > span_x && span_z > span_y, "tallest span is z (up)");
        assert!(
            span_y < span_x,
            "body depth (y) is the smallest lateral span"
        );
    }

    /// The orientation control must be EXACT — four quarter-turns about an axis return the asset
    /// bit-for-bit, or repeatedly nudging it would drift into the baked rig. And one turn about X
    /// stands a Y-up source up into the Z-up ground reckoning (ground = XY).
    #[test]
    fn quarter_turns_are_exact_and_stand_an_asset_up() {
        const IDENT: [f32; 16] = [
            1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
        ];
        // A Y-UP asset: its "up" runs along +Y, which is how a lot of vendor content arrives.
        let mut m = RawModel {
            vertices: vec![RawVertex {
                p: [0.0, 170.0, 0.0],
                n: [0.0, 1.0, 0.0],
                uv: [0.0, 0.0],
                joints: [0; 4],
                weights: [0.0; 4],
            }],
            indices: vec![0],
            bones: vec![RawBone {
                name: "root".into(),
                parent: -1,
                translation: [0.0, 0.0, 0.0],
                rotation: [0.0, 0.0, 0.0, 1.0],
                scale: [1.0; 3],
                inverse_bind: IDENT,
            }],
        };
        let before = m.vertices[0].p;

        apply_orientation(&mut m, quarter_turn(0));
        let p = m.vertices[0].p;
        assert!(
            (p[2] - 170.0).abs() < 1e-4 && p[1].abs() < 1e-4,
            "a turn about X stands the asset up along +Z, got {p:?}"
        );
        assert!(
            (m.vertices[0].n[2] - 1.0).abs() < 1e-4,
            "its normal came with it"
        );

        for _ in 0..3 {
            apply_orientation(&mut m, quarter_turn(0));
        }
        assert_eq!(
            m.vertices[0].p, before,
            "four quarter-turns are EXACTLY the identity"
        );
        assert_eq!(
            m.bones[0].inverse_bind, IDENT,
            "and the bone frames return too"
        );
    }
}
