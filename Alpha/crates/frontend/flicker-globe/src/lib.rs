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
pub mod worldmap;
pub use camera::OrbitCam;
pub use view::{Arrows, GlobeView, GLOBE_LAYERS, NO_ARROWS};
pub use world::{GlobeWorld, ShellSpec, DEFAULT_SET};
pub use worldmap::{
    HexSphereMap, MapBake, MapContent, MapExtent, MapFrame, MapLook, MapMode, WorldMap,
};

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
///
/// **An OVER-UNIT component means the cell EMITS**: radiance past 1 is
/// emission, so a colour closure returns e.g. `[1.3, 0.2, 0.06]` to make a
/// lava cell glow — bit 30 is set inside the escape and the mesh shader
/// draws the cell full-bright, unlit. Components are clamped for the stored
/// RGB either way.
fn direct(rgb: [f32; 3]) -> u32 {
    let q = |v: f32| ((v.clamp(0.0, 1.0) * 255.0).round() as u32) & 0xFF;
    let emissive = if rgb.iter().any(|v| *v > 1.0) {
        0x4000_0000
    } else {
        0
    };
    0x8000_0000 | emissive | q(rgb[0]) | (q(rgb[1]) << 8) | (q(rgb[2]) << 16)
}

/// [`direct`] with the emissive bit FORCED: the cell draws full-bright, unlit, at exactly
/// the stored colour. The flat [`WorldMap`] paints with this — a data map's ink must read
/// as the data's own value, not as that value under whatever rig the stage authors.
pub(crate) fn direct_emissive(rgb: [f32; 3]) -> u32 {
    direct(rgb) | 0x4000_0000
}

/// Linear blend of two RGB triples — the primitive every data-colour ramp on a
/// globe is made of. Lived as a private copy in two benches before moving here
/// (rule DDD070C7: the shared consumer moves the code).
pub fn lerp3(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    let t = t.clamp(0.0, 1.0);
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
    ]
}

/// Temperature ramp over a normalised value: cool deep-blue → red → white-hot.
///
/// **Relative to the field's own span, and that is the point.** A heat view's
/// job is to show WHERE the heat is — the interesting structure is often a few
/// hundred K riding on a 4000 K ball, and an absolute scale painted the whole
/// magma era one flat colour and told the maintainer nothing (the white ball,
/// 2026-08-06 — an era of uniform colour is an instrument reading blank).
/// THE heat ink for every bench: God Mode's interior fields and the Populous
/// molten seams read on the same ramp, so the same colour means the same thing.
pub fn temp_color(x: f32) -> [f32; 3] {
    let x = x.clamp(0.0, 1.0);
    if x < 0.5 {
        lerp3([0.10, 0.16, 0.55], [0.90, 0.35, 0.12], x * 2.0)
    } else {
        lerp3([0.90, 0.35, 0.12], [1.0, 0.95, 0.85], (x - 0.5) * 2.0)
    }
}

/// WATER temperature ramp (Aaron 2026-08-25): cold deep blue → nearly-white
/// ice blue → PURPLE for the hottest water (surface currents). THE ink the
/// water layers wear once their temperature tracking and circulation land —
/// stated now, beside the rock ramp, so the two cannot drift apart when the
/// erosion era starts reading both.
pub fn water_temp_color(x: f32) -> [f32; 3] {
    let x = x.clamp(0.0, 1.0);
    if x < 0.5 {
        lerp3([0.03, 0.10, 0.40], [0.82, 0.92, 0.98], x * 2.0)
    } else {
        lerp3([0.82, 0.92, 0.98], [0.55, 0.20, 0.75], (x - 0.5) * 2.0)
    }
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

/// Build one VOLUMETRIC shell: each cell a **closed solid** — a hex column with
/// its top face at `radius(i)`, its bottom face `depth(i)` beneath, and the six
/// (five at a pentagon) side walls between. The side edges lie along the corner
/// DIRECTIONS — straight lines out from the centre of the world — so the top
/// face is naturally a little wider than the bottom: a full stack of these
/// reads as the gently widening cone a radial column IS.
///
/// Same inputs, same sparseness and same inset semantics as [`build`]; the cap
/// shell and the column shell are two framings of one tiling, kept side by side
/// so neither is a fork of the other.
pub fn build_columns(
    dirs: &[Vec3],
    outlines: &[Vec<Vec3>],
    radius: impl Fn(usize) -> f32,
    depth: impl Fn(usize) -> f32,
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
        let r_top = radius(i);
        // A column is a solid: zero or negative depth would degenerate the side
        // walls, and a bottom through the origin would flip them inside out.
        let r_bot = (r_top - depth(i).max(1e-3)).max(r_top * 1e-3);
        let material = direct(rgb);
        let n = outline.len();
        // The corner directions, inset applied exactly as the cap shell does —
        // these ARE the side edges, shared by both rings.
        let corners: Vec<Vec3> = outline
            .iter()
            .map(|c| (*c * (1.0 - inset) + outward * inset).normalize())
            .collect();

        // One face at each end: a fan around the cell centre, the top facing
        // outward and the bottom facing back toward the world's centre.
        for (r, out) in [(r_top, outward), (r_bot, -outward)] {
            let base = verts.len() as u32;
            let center = outward * r;
            let normal = out.to_array();
            verts.push(MeshVertex {
                position: center.to_array(),
                normal,
                material,
            });
            for c in &corners {
                verts.push(MeshVertex {
                    position: (*c * r).to_array(),
                    normal,
                    material,
                });
            }
            for k in 0..n {
                let c0 = corners[k] * r;
                let c1 = corners[(k + 1) % n] * r;
                let i0 = base + 1 + k as u32;
                let i1 = base + 1 + ((k + 1) % n) as u32;
                // Wind so the triangle faces `out` (CCW front, back-culled).
                if (c0 - center).cross(c1 - center).dot(out) >= 0.0 {
                    indices.extend([base, i0, i1]);
                } else {
                    indices.extend([base, i1, i0]);
                }
            }
        }

        // The side walls: one quad per edge, its own flat normal (a wall is a
        // face, not a curve), wound to face away from the column's axis.
        for k in 0..n {
            let (ca, cb) = (corners[k], corners[(k + 1) % n]);
            let (b0, b1) = (ca * r_bot, cb * r_bot);
            let (t0, t1) = (ca * r_top, cb * r_top);
            let mid = (ca + cb) * 0.5;
            // The tangential outward direction at this wall — what "away from
            // the axis" means for a radial column.
            let out = (mid - outward * mid.dot(outward)).normalize_or_zero();
            // The natural order (b0, b1, t1) fronts along its own cross
            // product; REVERSE it only when that points inward — the same
            // per-triangle decision the fans make, so either ring order works.
            let g = (b1 - b0).cross(t0 - b0);
            let flip = g.dot(out) < 0.0;
            let normal = out.to_array();
            let base = verts.len() as u32;
            for p in [b0, b1, t1, t0] {
                verts.push(MeshVertex {
                    position: p.to_array(),
                    normal,
                    material,
                });
            }
            if flip {
                indices.extend([base, base + 2, base + 1, base, base + 3, base + 2]);
            } else {
                indices.extend([base, base + 1, base + 2, base, base + 2, base + 3]);
            }
        }
    }

    (verts, indices)
}

/// A cell's outline rotated into the COLUMN-LOCAL frame: the frame whose +Y is
/// the cell's own radial direction, so a column built from the result stands
/// upright at the top of a local sphere. What an inspector view feeds
/// [`build_columns`] to show ONE column on its own, upright, wherever on the
/// planet the cell actually sits.
pub fn column_frame(dir: Vec3, outline: &[Vec3]) -> Vec<Vec3> {
    let rot = glam::Quat::from_rotation_arc(dir.normalize_or_zero(), Vec3::Y);
    outline.iter().map(|c| rot * *c).collect()
}

/// The width of a cell across its corners at `radius` — twice the mean chord
/// from the cell's centre to its corners. The one number a viewport frames a
/// column by, answered by the tiling itself rather than a written-down copy.
pub fn tile_width(dir: Vec3, outline: &[Vec3], radius: f32) -> f32 {
    if outline.is_empty() {
        return 0.0;
    }
    let center = dir * radius;
    let sum: f32 = outline
        .iter()
        .map(|c| (*c * radius - center).length())
        .sum();
    2.0 * sum / outline.len() as f32
}

/// A stable, distinct hue per persistent plate id (golden-ratio rotation) —
/// a raft reads as a raft, and its colour never flickers as it drifts.
/// Diffuse / unassigned lithosphere (id 0) is neutral grey. Lifted from God
/// Mode when Populous grew the same motion-arrow field (rule DDD070C7).
pub fn plate_color(id: u32) -> [f32; 3] {
    if id == 0 {
        return [0.22, 0.23, 0.26];
    }
    let h = (id as f32 * 0.618_034).fract() * std::f32::consts::TAU;
    [
        0.45 + 0.4 * h.cos(),
        0.45 + 0.4 * (h + 2.094).cos(),
        0.45 + 0.4 * (h + 4.188).cos(),
    ]
}

/// Deterministic per-cell stipple: does cell `i` carry pattern `k` at this
/// coverage? A hash, not a random draw, so a sampled overlay (motion arrows,
/// air veils) stays stable frame to frame instead of crawling.
pub fn stippled(i: usize, k: usize, coverage: f64) -> bool {
    let h = (i as u32)
        .wrapping_mul(2_654_435_761)
        .wrapping_add(k as u32 * 97);
    ((h >> 8) % 1000) < (coverage * 1000.0) as u32
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

    /// **A column is a CLOSED solid with radial walls.** One hex cell through
    /// [`build_columns`]: a top fan, a bottom fan and six side quads — 38
    /// vertices, 72 indices — every side edge lying ALONG its corner's own
    /// direction (a straight line out from the centre of the world), which is
    /// exactly why the top face comes out wider than the bottom: the gently
    /// widening cone a radial column is.
    #[test]
    fn a_column_is_a_closed_solid_with_radial_walls() {
        let dirs = vec![Vec3::Y];
        let ring: Vec<Vec3> = (0..6)
            .map(|k| {
                let a = (k as f32 / 6.0) * std::f32::consts::TAU;
                Vec3::new(a.cos() * 0.2, 1.0, a.sin() * 0.2).normalize()
            })
            .collect();
        let outlines = vec![ring.clone()];
        let (v, i) = build_columns(
            &dirs,
            &outlines,
            |_| 100.0,
            |_| 10.0,
            0.0,
            |_| Some([1.0; 3]),
        );
        assert_eq!(v.len(), (1 + 6) + (1 + 6) + 4 * 6, "two fans + six walls");
        assert_eq!(i.len(), 3 * 6 + 3 * 6 + 6 * 6, "closed on every face");

        // Both rings ride the SAME corner directions at their own radius — the
        // radial-wall law — so the top ring is measurably the wider one.
        let top_c = Vec3::from_array(v[0].position);
        let bot_c = Vec3::from_array(v[7].position);
        assert!((top_c.length() - 100.0).abs() < 1e-3);
        assert!((bot_c.length() - 90.0).abs() < 1e-3);
        for k in 0..6 {
            let t = Vec3::from_array(v[1 + k].position);
            let b = Vec3::from_array(v[8 + k].position);
            assert!(
                t.normalize().dot(b.normalize()) > 1.0 - 1e-6,
                "corner {k}: both rings on one radial line"
            );
            assert!((t.length() - 100.0).abs() < 1e-3 && (b.length() - 90.0).abs() < 1e-3);
        }
        let width = |c: Vec3, first: Vec3| (first - c).length();
        assert!(
            width(top_c, Vec3::from_array(v[1].position))
                > width(bot_c, Vec3::from_array(v[8].position)),
            "the top hex is wider than the bottom"
        );

        // **Every face fronts the way its normal says** — the winding gate.
        // A back-culled renderer shows a triangle only from the side its
        // vertex order fronts; a wall wound inward is a wall you see through
        // (found in-window 2026-08-25: the flip test was inverted and the
        // whole column drew inside-out). For each emitted triangle the
        // geometric front, `(v1−v0)×(v2−v0)`, must agree with the stored
        // vertex normal — top, bottom and all six walls, both ring orders.
        for ring_reversed in [false, true] {
            let mut o = ring.clone();
            if ring_reversed {
                o.reverse();
            }
            let (v, i) = build_columns(&dirs, &[o], |_| 100.0, |_| 10.0, 0.0, |_| Some([1.0; 3]));
            for tri in i.chunks(3) {
                let p = |k: u32| Vec3::from_array(v[k as usize].position);
                let front = (p(tri[1]) - p(tri[0])).cross(p(tri[2]) - p(tri[0]));
                let n = Vec3::from_array(v[tri[0] as usize].normal);
                assert!(
                    front.dot(n) > 0.0,
                    "reversed({ring_reversed}) tri {tri:?} fronts against its normal"
                );
            }
        }

        // Sparseness is the same contract as the cap shell: no colour, no cell.
        let (empty, none) = build_columns(&dirs, &outlines, |_| 100.0, |_| 10.0, 0.0, |_| None);
        assert!(empty.is_empty() && none.is_empty());
    }

    /// **`column_frame` stands a cell upright, and `tile_width` measures the
    /// tiling itself.** The rotated outline surrounds +Y wherever the cell sat,
    /// preserving its shape; the width is twice the mean centre-to-corner chord
    /// at the asked radius.
    #[test]
    fn column_frame_stands_the_cell_upright_and_tile_width_measures_it() {
        let dir = Vec3::new(1.0, 1.0, 0.3).normalize();
        let ring: Vec<Vec3> = (0..6)
            .map(|k| {
                let a = (k as f32 / 6.0) * std::f32::consts::TAU;
                // A ring around `dir`, built from any two orthogonal partners.
                let u = dir.cross(Vec3::Y).normalize();
                let w = dir.cross(u);
                (dir + (u * a.cos() + w * a.sin()) * 0.1).normalize()
            })
            .collect();
        let local = column_frame(dir, &ring);
        for (k, c) in local.iter().enumerate() {
            assert!(
                (c.length() - 1.0).abs() < 1e-5,
                "corner {k} stays on the unit sphere"
            );
            let orig = ring[k].dot(dir).acos();
            let now = c.dot(Vec3::Y).acos();
            assert!(
                (orig - now).abs() < 1e-5,
                "corner {k} keeps its angular offset — the shape is preserved"
            );
        }
        let w = tile_width(dir, &ring, 200.0);
        let mean: f32 = ring
            .iter()
            .map(|c| (*c * 200.0 - dir * 200.0).length())
            .sum::<f32>()
            / 6.0;
        assert!((w - 2.0 * mean).abs() < 1e-4);
        assert_eq!(tile_width(dir, &[], 200.0), 0.0, "no ring, no width");
    }

    /// **The water-temperature ramp runs cold-blue → ice-white → hot-purple**
    /// (Aaron's ordering, verbatim) — three distinct stations, so a
    /// temperature field painted with it can never read as the rock ramp.
    #[test]
    fn the_water_ramp_runs_blue_ice_purple() {
        let cold = water_temp_color(0.0);
        let ice = water_temp_color(0.5);
        let hot = water_temp_color(1.0);
        assert!(cold[2] > cold[0] * 3.0, "cold is deep BLUE");
        assert!(
            ice.iter().all(|c| *c > 0.8),
            "the middle is nearly white ice blue"
        );
        assert!(
            hot[0] > hot[1] && hot[2] > hot[1],
            "hot is PURPLE — red and blue over green"
        );
        assert_eq!(water_temp_color(-1.0), cold, "clamped below");
        assert_eq!(water_temp_color(2.0), hot, "clamped above");
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
