//! Cinematic mesh helpers for the solar-birth scene — a colour-carrying UV sphere
//! and a banded ring annulus — plus a re-export of the shared [`flicker_orrery`]
//! planet layout the scene draws.
//!
//! The **planet-layout model** (the roster, orbits, kinds, periods) lives in the
//! GPU-free [`flicker_orrery`] crate so the heliocentric sky can share it — one
//! source of truth, never copied. This module keeps only what needs the GPU mesh
//! type: the sphere/ring builders and the direct-RGB colour packing they use. The
//! re-export lets existing callers keep saying `system::roster` / `system::Planet`.

use flicker::render::MeshVertex;

pub use flicker_orrery::*;

/// Pack an RGB colour into the mesh shader's direct-RGB escape (`mesh.wgsl`
/// `material_color`, u8-catalog layout 2026-08-19): bit 31 set, then RGB888 in
/// bits 0-7 (R) / 8-15 (G) / 16-23 (B). Lets a mesh carry a literal colour
/// instead of a material-catalog id.
pub fn pack_rgb(c: [f32; 3]) -> u32 {
    let q = |x: f32| (((x.clamp(0.0, 1.0) * 255.0) + 0.5) as u32) & 0xFF;
    0x8000_0000u32 | q(c[0]) | (q(c[1]) << 8) | (q(c[2]) << 16)
}

/// A unit UV sphere, every vertex carrying the flat surface colour `color` (unlit;
/// the sun point light shades it) and an outward normal. Wound CCW-outward to match
/// the mesh pipeline's back-face cull (`FrontFace::Ccw`, `cull Back`).
pub fn uv_sphere(color: [f32; 3], sectors: u32, stacks: u32) -> (Vec<MeshVertex>, Vec<u32>) {
    use std::f32::consts::PI;
    let mat = pack_rgb(color);
    let stride = sectors + 1;
    let mut verts = Vec::with_capacity(((stacks + 1) * stride) as usize);
    for i in 0..=stacks {
        let phi = PI * i as f32 / stacks as f32; // 0 (north pole) → PI (south)
        let (sp, cp) = phi.sin_cos();
        for j in 0..=sectors {
            let theta = std::f32::consts::TAU * j as f32 / sectors as f32;
            let (st, ct) = theta.sin_cos();
            let n = [sp * ct, cp, sp * st];
            verts.push(MeshVertex {
                position: n,
                normal: n,
                material: mat,
            });
        }
    }
    let mut idx = Vec::with_capacity((stacks * sectors * 6) as usize);
    for i in 0..stacks {
        for j in 0..sectors {
            let a = i * stride + j;
            let b = a + stride;
            // CCW-outward (verified against the mesh pipeline's back-face cull).
            idx.extend_from_slice(&[a, a + 1, b, a + 1, b + 1, b]);
        }
    }
    (verts, idx)
}

/// A flat banded ring annulus in the local XZ plane (`y = 0`), radii in
/// `[inner, outer]` (units of the planet radius). Concentric greyscale brightness
/// bands (a Cassini-division feel); the caller tilts, scales, and **tints** it.
/// Double-sided so it shows from either face. Salvaged from the old `planet.rs`.
pub fn ring_mesh(
    inner: f32,
    outer: f32,
    segments: usize,
    bands: usize,
) -> (Vec<MeshVertex>, Vec<u32>) {
    use std::f32::consts::TAU;
    let stride = segments + 1;
    let mut verts = Vec::with_capacity((bands + 1) * stride);
    for bi in 0..=bands {
        let r = inner + (outer - inner) * bi as f32 / bands as f32;
        let b = 0.45 + 0.55 * (0.5 + 0.5 * (bi as f32 * 2.7).sin()); // concentric bands / gaps
        let m = pack_rgb([b, b, b]);
        for si in 0..=segments {
            let a = si as f32 / segments as f32 * TAU;
            let (s, c) = a.sin_cos();
            verts.push(MeshVertex {
                position: [r * c, 0.0, r * s],
                normal: [0.0, 1.0, 0.0],
                material: m,
            });
        }
    }
    let mut idx = Vec::with_capacity(bands * segments * 12);
    for bi in 0..bands {
        for si in 0..segments {
            let a = (bi * stride + si) as u32;
            let b = a + stride as u32;
            idx.extend_from_slice(&[a, b, a + 1, a + 1, b, b + 1]); // front
            idx.extend_from_slice(&[a + 1, b, a, b + 1, b, a + 1]); // back (double-sided)
        }
    }
    (verts, idx)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn direct_rgb_escape_round_trips() {
        let m = pack_rgb([1.0, 0.0, 0.5]);
        assert_ne!(m & 0x8000_0000, 0, "direct-RGB escape flag (bit 31) set");
        assert_eq!(m & 0xFF, 255, "R = full");
        assert_eq!((m >> 8) & 0xFF, 0, "G = none");
        assert_eq!((m >> 16) & 0xFF, 128, "B ≈ half");
    }
}
