//! The ring annulus mesh and the RGB→`material` packer for the composed hex globes.
//!
//! Every body (and moon) is a flicker-world hex globe (`worldglobe`), composition-coloured and
//! lit by the engine star **point light** — one consistent scheme, no per-body surface code
//! here. This module only supplies [`ring_mesh`] (the annulus drawn around ring-bearing giants)
//! and [`pack_rgb`] (packs an RGB colour into the mesh shader's direct-RGB666 `material` escape).

use flicker::render::MeshVertex;

/// A flat banded ring annulus in the local XZ plane (`y = 0`), radii in `[inner, outer]` (units
/// of the planet radius). Concentric brightness bands (Cassini-division feel) in greyscale —
/// the renderer tilts, scales, and **tints** it per giant via `MeshDrawOptions`. Double-sided so
/// it shows from either face. Returns `(vertices, indices)`.
pub fn ring_mesh(inner: f32, outer: f32, segments: usize, bands: usize) -> (Vec<MeshVertex>, Vec<u32>) {
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
            verts.push(MeshVertex { position: [r * c, 0.0, r * s], normal: [0.0, 1.0, 0.0], material: m });
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

/// Pack RGB into the mesh shader's direct-RGB666 escape: low 12 bits = `0xFFF`, then 6-bit
/// channels in bits 12-17 (R) / 18-23 (G) / 24-29 (B).
pub(crate) fn pack_rgb(c: [f32; 3]) -> u32 {
    let q = |x: f32| (((x.clamp(0.0, 1.0) * 63.0) + 0.5) as u32) & 0x3F;
    0xFFFu32 | (q(c[0]) << 12) | (q(c[1]) << 18) | (q(c[2]) << 24)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn direct_rgb_escape_round_trips() {
        // The packed value must carry the 0xFFF escape and recover the channels (as the shader does).
        let m = pack_rgb([1.0, 0.0, 0.5]);
        assert_eq!(m & 0xFFF, 0xFFF, "direct-RGB escape marker set");
        assert_eq!((m >> 12) & 0x3F, 63, "R = full");
        assert_eq!((m >> 18) & 0x3F, 0, "G = none");
        assert_eq!((m >> 24) & 0x3F, 32, "B ≈ half (0.5·63 ≈ 32)");
    }
}
