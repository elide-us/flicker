//! One shell of the layer stack: a mesh of hex patches on a sphere of a given
//! radius. The scene builds **one of these per layer** (core, mantle, each crust
//! bed, …) and stacks them radially — each layer is its own mesh, drawn where it
//! exists (§ the "build the hex model as the visualization stack" design). The
//! `color` closure returns `None` for a cell that does **not** carry this layer, so
//! a shell is **sparse**: a crust shell has holes where the mantle is still bare.

use flicker::render::{MeshVertex, Vec3};

/// Base globe radius (world units) — the outermost shell; the orbit camera frames
/// this. Inner shells (mantle, core) are drawn at fractions of it.
pub const RADIUS: f32 = 200.0;

/// Is this direction inside the cutaway wedge? A 90° quadrant of longitude, so a
/// shell built with the cutaway on has a quarter missing and the shells *below* it
/// show through in section — the MRI slice through the stack. The innermost shell
/// is never cut (there is nothing beneath it to reveal).
pub fn in_wedge(dir: Vec3) -> bool {
    dir.x > 0.0 && dir.z > 0.0
}

/// The shader's direct-RGB material word (`mesh.wgsl`: primary `0xFFF` escape,
/// RGB666) — lets us colour a cell without a material-table entry.
fn direct(rgb: [f32; 3]) -> u32 {
    let q = |v: f32| ((v.clamp(0.0, 1.0) * 63.0).round() as u32) & 0x3F;
    0xFFF | (q(rgb[0]) << 12) | (q(rgb[1]) << 18) | (q(rgb[2]) << 24)
}

/// Build one shell mesh at `radius` from cell centres (`dirs`) + boundary outlines,
/// colouring each cell by `color(i)`. A cell whose `color` returns `None` is
/// **skipped** — that is how a layer shell is drawn only where the layer exists.
pub fn build(
    dirs: &[Vec3],
    outlines: &[Vec<Vec3>],
    radius: f32,
    color: impl Fn(usize) -> Option<[f32; 3]>,
) -> (Vec<MeshVertex>, Vec<u32>) {
    let mut verts: Vec<MeshVertex> = Vec::new();
    let mut indices: Vec<u32> = Vec::new();

    for (i, outline) in outlines.iter().enumerate() {
        if outline.len() < 3 || i >= dirs.len() {
            continue;
        }
        let Some(rgb) = color(i) else {
            continue; // this cell has no such layer — leave a hole
        };
        let outward = dirs[i];
        let r = radius;
        let material = direct(rgb);
        let normal = outward.to_array();
        let center = outward * r;

        let base = verts.len() as u32;
        verts.push(MeshVertex { position: center.to_array(), normal, material });
        for corner in outline {
            verts.push(MeshVertex { position: (*corner * r).to_array(), normal, material });
        }
        let n = outline.len();
        for k in 0..n {
            let c0 = outline[k] * r;
            let c1 = outline[(k + 1) % n] * r;
            let i0 = base + 1 + k as u32;
            let i1 = base + 1 + ((k + 1) % n) as u32;
            // Wind so the triangle faces outward (CCW front, back-culled).
            if (c0 - center).cross(c1 - center).dot(outward) >= 0.0 {
                indices.extend([base, i0, i1]);
            } else {
                indices.extend([base, i1, i0]);
            }
        }
    }

    (verts, indices)
}
