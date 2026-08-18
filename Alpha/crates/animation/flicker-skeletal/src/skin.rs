//! CPU linear-blend skinning (Slice 2).
//!
//! The pose layer (`pose.rs`) is authoritative; this turns the posed global bone
//! transforms into deformed mesh vertices on the CPU each frame. Positions/normals
//! stay in SOURCE space — the viewer's `world` model matrix (source→engine) is
//! applied by the mesh pipeline, exactly as it is to the skeleton lines, so the
//! skinned mesh and the bone wireframe register perfectly.
//!
//! Per-frame CPU skin + re-upload is fine for one POC character (per the brief —
//! do not prematurely optimize). The GPU-palette split is an alpha step.

use glam::{Mat3, Mat4, Vec3, Vec4};

use crate::format::{Bone, Mesh};

/// Skinning palette: `palette[b] = global[b] * inverse_bind[b]`. Maps a bind-pose
/// vertex (source space) to its posed position (source space).
pub fn palette(bones: &[Bone], globals: &[Mat4]) -> Vec<Mat4> {
    bones
        .iter()
        .zip(globals.iter())
        .map(|(b, g)| *g * b.inverse_bind)
        .collect()
}

/// A CPU-skinned vertex (deformed position + normal, source space). UVs are static
/// and read straight from the mesh, so they're not carried here.
#[derive(Copy, Clone)]
pub struct SkinnedVertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
}

/// 4-influence linear-blend skinning of every mesh vertex, parallel to
/// `mesh.vertices`. The caller slices this per submesh and builds textured or flat
/// GPU vertices from it. No morphs — see `skin_morphed` for the create-a-face path.
pub fn skin(mesh: &Mesh, palette: &[Mat4]) -> Vec<SkinnedVertex> {
    skin_morphed(mesh, palette, &[])
}

/// As `skin`, but first blends the mesh's facial morph targets by `morph_weights` (parallel to
/// `mesh.morphs`; short/empty → treated as 0) into the bind positions — the create-a-face path.
/// Morphs reshape the bind face, then skinning poses it. `&[]` weights are byte-identical to
/// `skin`; the blend is sparse, so only verts a nonzero morph touches cost anything.
pub fn skin_morphed(mesh: &Mesh, palette: &[Mat4], morph_weights: &[f32]) -> Vec<SkinnedVertex> {
    let active = mesh
        .morphs
        .iter()
        .zip(morph_weights)
        .any(|(_, &w)| w != 0.0);
    let morphed = active.then(|| apply_morphs(mesh, morph_weights));
    mesh.vertices
        .iter()
        .enumerate()
        .map(|(i, v)| {
            let base = morphed.as_ref().map_or(v.p, |p| p[i]);
            let p = Vec4::new(base[0], base[1], base[2], 1.0);
            let n = Vec3::from(v.n);
            let mut pos = Vec3::ZERO;
            let mut nrm = Vec3::ZERO;
            for k in 0..4 {
                let w = v.weights[k];
                if w == 0.0 {
                    continue;
                }
                let m = palette
                    .get(v.joints[k] as usize)
                    .copied()
                    .unwrap_or(Mat4::IDENTITY);
                pos += w * (m * p).truncate();
                nrm += w * (Mat3::from_mat4(m) * n);
            }
            let nrm = if nrm.length_squared() > 1e-12 {
                nrm.normalize()
            } else {
                Vec3::Y
            };
            SkinnedVertex {
                position: pos.to_array(),
                normal: nrm.to_array(),
            }
        })
        .collect()
}

/// Blend the mesh's morph targets into a fresh copy of the bind vertex positions, weighted by
/// `morph_weights` (parallel to `mesh.morphs`). Sparse — only listed verts move. Returned
/// positions are parallel to `mesh.vertices`. Public so the create-a-face UI can preview a
/// reshaped face without skinning it.
pub fn apply_morphs(mesh: &Mesh, morph_weights: &[f32]) -> Vec<[f32; 3]> {
    let mut pos: Vec<[f32; 3]> = mesh.vertices.iter().map(|v| v.p).collect();
    for (m, &w) in mesh.morphs.iter().zip(morph_weights) {
        if w == 0.0 {
            continue;
        }
        for md in &m.deltas {
            if let Some(p) = pos.get_mut(md.v as usize) {
                p[0] += w * md.d[0];
                p[1] += w * md.d[1];
                p[2] += w * md.d[2];
            }
        }
    }
    pos
}

#[cfg(test)]
mod tests {
    use super::{apply_morphs, skin, skin_morphed};
    use crate::format::{Mesh, Morph, MorphDelta, Vertex};
    use glam::Mat4;

    fn vert(p: [f32; 3]) -> Vertex {
        Vertex {
            p,
            n: [0.0, 1.0, 0.0],
            uv: [0.0, 0.0],
            joints: [0, 0, 0, 0],
            weights: [1.0, 0.0, 0.0, 0.0],
        }
    }

    #[test]
    fn morph_blend_displaces_only_targeted_verts_scaled_by_weight() {
        let mesh = Mesh {
            vertices: vec![vert([0.0, 0.0, 0.0]), vert([1.0, 0.0, 0.0])],
            morphs: vec![Morph {
                name: "wider".into(),
                deltas: vec![MorphDelta {
                    v: 1,
                    d: [10.0, 0.0, 0.0],
                }],
            }],
            ..Default::default()
        };
        // weight 0 → nothing moves; and empty weights are byte-identical to `skin`.
        assert_eq!(apply_morphs(&mesh, &[0.0])[1], [1.0, 0.0, 0.0]);
        assert_eq!(skin(&mesh, &[Mat4::IDENTITY])[1].position, [1.0, 0.0, 0.0]);
        // weight 0.5 → half the delta, on the targeted vert only.
        let p = apply_morphs(&mesh, &[0.5]);
        assert_eq!(p[0], [0.0, 0.0, 0.0], "an untargeted vert is untouched");
        assert_eq!(p[1], [6.0, 0.0, 0.0], "1.0 + 0.5*10");
        // full weight, skinned through identity == the morphed position.
        assert_eq!(
            skin_morphed(&mesh, &[Mat4::IDENTITY], &[1.0])[1].position,
            [11.0, 0.0, 0.0]
        );
    }
}
