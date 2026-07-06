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
/// GPU vertices from it.
pub fn skin(mesh: &Mesh, palette: &[Mat4]) -> Vec<SkinnedVertex> {
    mesh.vertices
        .iter()
        .map(|v| {
            let p = Vec4::new(v.p[0], v.p[1], v.p[2], 1.0);
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
