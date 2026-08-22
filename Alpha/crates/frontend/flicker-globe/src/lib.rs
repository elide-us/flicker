//! **One hex-globe mesh builder, shared.** A shell of hex patches on a sphere of
//! a given radius: a scene stacks them radially (core, mantle, each crust bed, …)
//! and each is drawn only where it exists. The `color` closure returns `None` for
//! a cell that does not carry this layer, so a shell is **sparse** — a crust shell
//! has holes where the mantle is still bare.
//!
//! Lifted out of `flicker-godmode` when the Populous bench needed the same mesh
//! and would have been the THIRD copy. The third copy — `flicker-pocepochs`'s
//! own `globe.rs` + `camera.rs` — is **gone**: its per-cell radius variant was
//! absorbed here (see [`build`]'s `radius` closure and
//! [`ShellSpec::cell_radius`]), and the bench now asks [`GlobeWorld`] for its
//! planet like every other. There is ONE globe in Prism, and this is it.
//!
//! Kept that way by gates on both sides of the seam:
//! `a_per_cell_radius_shell_matches_the_sphere_vertex_for_vertex` (below) holds
//! the absorbed framing to the geometry budget of the sphere it replaced, and
//! `no_scene_reads_a_device_or_names_a_pane_style` (in `flicker-widgets`) fails
//! the moment any scene grows its own shell builder or orbit camera again.

pub mod camera;
pub mod view;
pub mod world;
pub use camera::OrbitCam;
pub use view::{Arrows, GlobeStage, GlobeView, StageLayer, NO_ARROWS};
pub use world::{GlobeWorld, ShellSpec};

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

/// The shader's direct-RGB material word (`mesh.wgsl`: bit-31 escape, RGB888
/// in bits 0-23; u8-catalog layout 2026-08-19) — lets us colour a cell without
/// a material-catalog id.
fn direct(rgb: [f32; 3]) -> u32 {
    let q = |v: f32| ((v.clamp(0.0, 1.0) * 255.0).round() as u32) & 0xFF;
    0x8000_0000 | q(rgb[0]) | (q(rgb[1]) << 8) | (q(rgb[2]) << 16)
}

/// Build one shell mesh from cell centres (`dirs`) + boundary outlines,
/// colouring each cell by `color(i)`. A cell whose `color` returns `None` is
/// **skipped** — that is how a layer shell is drawn only where the layer exists.
///
/// `radius` is asked **per cell**. A sphere passes `|_| r`; a stack whose top
/// follows each column's own accumulated thickness passes the column's height.
/// That second form is the variant `flicker-pocepochs` carried privately until
/// it was absorbed here — one builder, two framings, no second copy.
///
/// `inset` (0..1) pulls every corner toward its cell centre by that fraction —
/// `0.0` is the exact tiling. An inset shell drawn OVER a full shell at a
/// slightly smaller radius shows the under-shell's colour through the gaps:
/// that is how a hex map gets uniform OUTLINES without a per-edge line mesh or
/// a per-frame line submit — two static shells, and the seams are the lines.
pub fn build(
    dirs: &[Vec3],
    outlines: &[Vec<Vec3>],
    radius: impl Fn(usize) -> f32,
    inset: f32,
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
        let r = radius(i);
        let material = direct(rgb);
        let normal = outward.to_array();
        let center = outward * r;

        let base = verts.len() as u32;
        verts.push(MeshVertex {
            position: center.to_array(),
            normal,
            material,
        });
        for corner in outline {
            // Blend toward the centre direction, then re-project to the sphere,
            // so an inset corner still sits ON the shell rather than inside it.
            let dir = (*corner * (1.0 - inset) + outward * inset).normalize();
            verts.push(MeshVertex {
                position: (dir * r).to_array(),
                normal,
                material,
            });
        }
        let n = outline.len();
        for k in 0..n {
            let c0 = outline[k] * r;
            let c1 = outline[(k + 1) % n] * r;
            let i0 = base + 1 + k as u32;
            let i1 = base + 1 + ((k + 1) % n) as u32;
            // Wind so the triangle faces outward (CCW front, back-culled). The
            // ORIGINAL corners decide the winding — an inset preserves
            // orientation, so the test needs no re-derivation.
            if (c0 - center).cross(c1 - center).dot(outward) >= 0.0 {
                indices.extend([base, i0, i1]);
            } else {
                indices.extend([base, i1, i0]);
            }
        }
    }

    (verts, indices)
}

// ── the graticule — THE reference frame every globe draws ───────────────────

/// Segments per full circle. Enough that a great circle reads as a curve rather
/// than a polygon at any zoom the benches allow.
const GRID_STEPS: usize = 144;
/// Axial tilt, degrees — the tropics and the polar circles are this angle
/// measured from the equator and from the poles. Prism's own tilt: the number
/// that decides where the sun stands overhead and where it never rises.
const AXIAL_TILT_DEG: f32 = 23.44;
/// Spacing of the ordinary parallels and meridians, degrees.
const GRID_SPACING_DEG: f32 = 30.0;

/// The reference frame: parallels, meridians, and the four latitudes that mean
/// something — ONE graticule, drawn identically over every bench's globe
/// (lifted from God Mode when Populous needed the same frame; a second grid
/// would drift in colour, tilt or spacing the day one of them was retuned).
///
/// **The equator, the tropics and the polar circles are not decoration** — the
/// insolation law reads latitude straight off the Y axis, so those lines mark
/// exactly where the surface temperature bands, the evaporation and the ice
/// actually change. The prime meridian is +X and the antimeridian −X by
/// declaration, which is all a prime meridian ever is: Greenwich is a choice,
/// not a discovery.
///
/// `radius` is the DRAW radius — pass the shell radius times a clearance
/// factor (God Mode uses `1.022 ×`) so the grid frames the world rather than
/// lying on it. Grouped by colour for [`view::GlobeView::render`]'s line
/// channel, which draws it depth-tested through the same pass as everything
/// else — a second line consumer, never a second line system.
pub fn graticule(radius: f32) -> view::Arrows {
    let ring = |lat_deg: f32| -> Vec<(Vec3, Vec3)> {
        let lat = lat_deg.to_radians();
        let (y, r) = (lat.sin(), lat.cos());
        let at = |k: usize| {
            let a = k as f32 / GRID_STEPS as f32 * std::f32::consts::TAU;
            Vec3::new(r * a.cos(), y, r * a.sin()) * radius
        };
        (0..GRID_STEPS).map(|k| (at(k), at(k + 1))).collect()
    };
    let meridian = |lon_deg: f32| -> Vec<(Vec3, Vec3)> {
        let lon = lon_deg.to_radians();
        let at = |k: usize| {
            let a = k as f32 / GRID_STEPS as f32 * std::f32::consts::TAU;
            Vec3::new(a.cos() * lon.cos(), a.sin(), a.cos() * lon.sin()) * radius
        };
        (0..GRID_STEPS).map(|k| (at(k), at(k + 1))).collect()
    };

    // The ordinary grid — dim, so it frames without competing.
    let faint = [0.42, 0.47, 0.58, 1.0];
    let mut mesh: Vec<(Vec3, Vec3)> = Vec::new();
    let mut lat = GRID_SPACING_DEG;
    while lat < 90.0 {
        mesh.extend(ring(lat));
        mesh.extend(ring(-lat));
        lat += GRID_SPACING_DEG;
    }
    let mut lon = GRID_SPACING_DEG;
    while lon < 180.0 {
        mesh.extend(meridian(lon));
        lon += GRID_SPACING_DEG;
    }

    vec![
        (faint, mesh),
        // The equator — the one line every other latitude is measured from.
        ([0.95, 0.80, 0.35, 1.0], ring(0.0)),
        // The tropics: the band the star can stand directly over.
        ([0.55, 0.85, 0.55, 1.0], {
            let mut v = ring(AXIAL_TILT_DEG);
            v.extend(ring(-AXIAL_TILT_DEG));
            v
        }),
        // The polar circles: where the sun can fail to rise at all.
        ([0.55, 0.75, 0.95, 1.0], {
            let mut v = ring(90.0 - AXIAL_TILT_DEG);
            v.extend(ring(-(90.0 - AXIAL_TILT_DEG)));
            v
        }),
        // Prime meridian and antimeridian — the seam the map is cut on.
        ([0.90, 0.55, 0.45, 1.0], meridian(0.0)),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **The one reference frame, structurally.** Five ink groups — grid,
    /// equator, tropics, polar circles, prime seam — every segment on the
    /// asked sphere, the equator flat at zero latitude. Both benches draw this
    /// same value, so this is the only place its shape needs asserting.
    #[test]
    fn the_graticule_is_the_shared_five_group_frame() {
        let r = 204.4;
        let groups = graticule(r);
        assert_eq!(groups.len(), 5, "grid · equator · tropics · polar · prime");
        let (_, grid) = &groups[0];
        // Parallels ±30/±60 (4 rings) + meridian circles 30..150 (5).
        assert_eq!(grid.len(), (4 + 5) * GRID_STEPS);
        let (_, equator) = &groups[1];
        assert_eq!(equator.len(), GRID_STEPS);
        for (a, b) in equator {
            assert!((a.length() - r).abs() < 0.05 && (b.length() - r).abs() < 0.05);
            assert!(a.y.abs() < 1e-3, "the equator sits at zero latitude");
        }
        assert_eq!(groups[2].1.len(), 2 * GRID_STEPS, "both tropics");
        assert_eq!(groups[3].1.len(), 2 * GRID_STEPS, "both polar circles");
        assert_eq!(
            groups[4].1.len(),
            GRID_STEPS,
            "the prime seam, one full circle"
        );
    }

    /// `inset` pulls corners toward the cell centre and `0.0` is the exact
    /// tiling — the two-shell outline trick depends on both halves.
    #[test]
    fn build_inset_pulls_corners_toward_the_centre() {
        let dirs = vec![Vec3::Y];
        let ring: Vec<Vec3> = (0..6)
            .map(|k| {
                let a = (k as f32 / 6.0) * std::f32::consts::TAU;
                Vec3::new(a.cos() * 0.2, 1.0, a.sin() * 0.2).normalize()
            })
            .collect();
        let outlines = vec![ring.clone()];
        let (exact, _) = build(&dirs, &outlines, |_| 10.0, 0.0, |_| Some([1.0; 3]));
        let (inset, _) = build(&dirs, &outlines, |_| 10.0, 0.2, |_| Some([1.0; 3]));
        // Vertex 0 is the centre; corners follow.
        let c0 = Vec3::from_array(exact[1].position);
        let c0_in = Vec3::from_array(inset[1].position);
        assert!(
            (c0 - ring[0] * 10.0).length() < 1e-3,
            "inset 0 is the exact tiling"
        );
        let centre = Vec3::from_array(exact[0].position);
        assert!(
            (c0_in - centre).length() < (c0 - centre).length(),
            "an inset corner moved toward the centre"
        );
        assert!(
            (c0_in.length() - 10.0).abs() < 1e-3,
            "and stayed on the shell"
        );
    }

    /// **The absorbed variant costs nothing and answers per column.** The third
    /// globe copy (`flicker-pocepochs/src/globe.rs`) differed from this builder
    /// in exactly one way: its radius was a closure, so a layer could stand at
    /// each column's own accumulated height. Absorbed, that framing must produce
    /// the IDENTICAL geometry budget — same vertex count, same index count, same
    /// winding — and differ only in where each cell's ring sits. A regression
    /// here means the migration changed the picture, which it must not.
    #[test]
    fn a_per_cell_radius_shell_matches_the_sphere_vertex_for_vertex() {
        let dirs = vec![Vec3::Y, Vec3::X, Vec3::Z];
        let ring = |axis: Vec3| -> Vec<Vec3> {
            (0..6)
                .map(|k| {
                    let a = (k as f32 / 6.0) * std::f32::consts::TAU;
                    (axis + Vec3::new(a.cos(), a.sin(), a.cos() * a.sin()) * 0.15).normalize()
                })
                .collect()
        };
        let outlines: Vec<Vec<Vec3>> = dirs.iter().map(|d| ring(*d)).collect();
        let all = |_: usize| Some([0.5; 3]);

        let (flat_v, flat_i) = build(&dirs, &outlines, |_| 100.0, 0.0, all);
        // Each column stands at its own height — the pocepochs stack read.
        let heights = [100.0f32, 120.0, 140.0];
        let (stack_v, stack_i) = build(&dirs, &outlines, |i| heights[i], 0.0, all);

        assert_eq!(
            stack_v.len(),
            flat_v.len(),
            "the same vertices as before the absorption"
        );
        assert_eq!(stack_i.len(), flat_i.len(), "and the same triangles");
        assert_eq!(
            stack_i, flat_i,
            "winding is decided by the outline, not the radius"
        );
        assert_eq!(
            flat_v.len(),
            dirs.len() * (1 + 6),
            "a centre plus its ring, per cell"
        );
        // Vertex 0 of each cell is its centre, at that cell's own radius.
        for (i, want) in heights.iter().enumerate() {
            let centre = Vec3::from_array(stack_v[i * 7].position);
            assert!(
                (centre.length() - want).abs() < 1e-2,
                "cell {i} stands at {want}"
            );
            for k in 1..7 {
                let corner = Vec3::from_array(stack_v[i * 7 + k].position);
                assert!(
                    (corner.length() - want).abs() < 1e-2,
                    "its whole ring rides with it"
                );
            }
        }
        // A constant closure IS the sphere — the two framings are one builder.
        let (same_v, _) = build(&dirs, &outlines, |_| 100.0, 0.0, all);
        for (a, b) in same_v.iter().zip(&flat_v) {
            assert_eq!(a.position, b.position);
        }
    }
}
