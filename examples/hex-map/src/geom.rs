//! Pure hex geometry shared by every part of the client: the flat-top hexagon,
//! the edge/ring math, the two maps' tile layout, and the translucent fill mesh.
//! No `Renderer` — these are the coordinate-frame primitives the map structures,
//! the snap map, and the world-gen all build on. Re-exported at the crate root
//! (`pub use geom::*`) so existing `crate::…` paths keep resolving.

use flicker::render::{Mat4, MeshVertex, Vec3};

/// Corner-to-corner width of the hex (east–west, point through point).
pub const HEX_WIDTH: f32 = 2048.0;
/// Circumradius = half the corner-to-corner width (centre → any corner).
pub const HEX_SIZE: f32 = HEX_WIDTH * 0.5;

/// Centre → edge-midpoint distance (apothem) = circumradius · √3/2.
pub const APOTHEM: f32 = HEX_SIZE * 0.866_025_4;
/// Empty space left between two neighbouring hexes' nearest edges.
pub const HEX_GAP: f32 = 384.0;
/// Centre-to-centre distance for edge-adjacent hexes, including the gap.
pub const HEX_SPACING: f32 = 2.0 * APOTHEM + HEX_GAP;

/// Empty world space between the two maps' nearest extents, so they never
/// overlap as the ring count grows.
const CLEAR_GAP: f32 = 2500.0;
/// How far south of a map's outer extent its roll wheel sits.
const WHEEL_MARGIN: f32 = 600.0;

/// How far a `rings`-ring map reaches from its centre — the point-to-centre
/// extent (`rings` whole steps plus the corner point). The separation and the
/// wheel position both scale off this so larger maps stay clear.
fn map_radius(rings: usize) -> f32 {
    rings as f32 * HEX_SPACING + HEX_SIZE
}

/// World X of the south (left) map's **centre column** for `rings` rings — the
/// separation between the two map centres (north at `x = 0`, south at `x =
/// sep`). The south map is mirror-flipped about, and rolls about, this column,
/// and its compass sits on it. It grows with the ring count (two map radii plus
/// a gap) so the maps stay apart at any ring count.
pub fn sep(rings: usize) -> f32 {
    2.0 * map_radius(rings) + CLEAR_GAP
}

/// World Z (south) of a map's roll wheel for `rings` rings — just south of the
/// map's outer extent, so a larger map never draws over its own wheel.
pub fn wheel_z(rings: usize) -> f32 {
    -(map_radius(rings) + WHEEL_MARGIN)
}

/// Material index of the hex face fill — CLOUD_MID, a neutral mid-grey in the
/// mesh palette (so the fill reads grey, not coloured).
pub const FILL_MATERIAL: u32 = 7;

/// Each map hex as a path of unit edge-steps from the origin (edge indices
/// `a`=0 … `f`=5), indexed by the hex's number. Hex 0 is the origin; 1–6 are
/// ring 1 (one step across each edge); 7–18 are ring 2, walked **clockwise from
/// due west**: hex 7 attaches to hex 6's edge `a` (so 6.a ↔ 7.d), hex 8 is the
/// step north of it (8.e ↔ 7.b), and so on round to hex 18 (SW), which closes
/// back onto hex 7. Each pair `[i, i]` lands on a ring-2 corner, `[i, i+1]` on
/// the edge-hex between two corners.
///
/// Reference/spec table: the layout is now grown from [`first_ring`] (which
/// reproduces this exactly), so this is consumed only by the `ring_formula` /
/// `ring2_spiral` tests that pin that equivalence.
#[allow(dead_code)]
pub const HEX_STEPS: [&[usize]; 19] = [
    &[], // 0  origin
    &[0], &[1], &[2], &[3], &[4], &[5], // 1–6: across edges a–f (NW,N,NE,SE,S,SW)
    &[5, 0], // 7  f+a  — due west
    &[0, 0], // 8  2a   — NW corner
    &[0, 1], // 9  a+b
    &[1, 1], // 10 2b   — N corner
    &[1, 2], // 11 b+c
    &[2, 2], // 12 2c   — NE corner
    &[2, 3], // 13 c+d  — due east
    &[3, 3], // 14 2d   — SE corner
    &[3, 4], // 15 d+e
    &[4, 4], // 16 2e   — S corner
    &[4, 5], // 17 e+f
    &[5, 5], // 18 2f   — SW corner
];

/// The "left map": tiles placed by the user's connection rules, each entry the
/// tile's *logical* path from the first map's origin. The whole chart is drawn
/// **record-flipped** about the south map's centre column (logical X reflects, and each tile
/// mirrors — `draw_hex(.., flip=true)`). It is a full 19-tile hexagon around the
/// centre at logical (√3/2, ½): ring-2 (19–30), ring-1 (31–36), centre (37).
/// Ring-2: seam/west column 19–21 (aligned with the first map's 18/7/8 at the
/// half-step); 22–23 curve up (`f` onto prev `c`); 24–25 over the top (`a` onto
/// prev `d`); 26–27 down the east edge (`b` onto prev `e`); 28–29 close the
/// south (`c` onto prev `f`); 30 turns NW and shuts it (30.a ↔ 19.d). Ring-1
/// (31–36) spirals in from 30, marches rotating N,N,NE,SE,S,SW and closes onto
/// 31. (Drawn as 19 + index.)
///
/// Reference/spec table: the layout is now grown from [`left_ring`] (which
/// reproduces this exactly), so this is consumed only by the `ring_formula` /
/// `left_map_column` tests that pin that equivalence.
#[allow(dead_code)]
pub const LEFT_MAP_STEPS: [&[usize]; 19] = [
    &[5, 5, 0], // 19  f+f+a — west edge
    &[0, 0, 5], // 20  a+a+f
    &[0, 0, 0], // 21  3a
    &[0, 0, 1], // 22  2a+b — 22.f ↔ 21.c  (north curve, up)
    &[0, 1, 1], // 23  a+2b — 23.f ↔ 22.c
    &[1, 1],    // 24  2b   — 24.a ↔ 23.d  (over the top)
    &[1, 2],    // 25  b+c  — 25.a ↔ 24.d
    &[2],       // 26  c    — 26.b ↔ 25.e  (east edge, down)
    &[3],       // 27  d    — 27.b ↔ 26.e
    &[4],       // 28  e    — 28.c ↔ 27.f  (south, closing)
    &[4, 5],    // 29  e+f  — 29.c ↔ 28.f
    &[5, 5],    // 30  2f   — 30.d ↔ 29.a; 30.a ↔ 19.d (ring-2 closes)
    &[0, 5],    // 31  f+a  — 31.e ↔ 30.b  (inner ring, north)
    &[0, 0],    // 32  2a   — 32.e ↔ 31.b
    &[0, 1],    // 33  a+b  — 33.f ↔ 32.c  (NE)
    &[1],       // 34  b    — 34.a ↔ 33.d  (SE)
    &[],        // 35  orig — 35.b ↔ 34.e  (S)
    &[5],       // 36  f    — 36.c ↔ 35.f  (SW); closes onto 31
    &[0],       // 37  a    — centre tile, at (√3/2, ½)
];

// ───────────────────────────────────────────────────────────────────
// Geometry helpers
// ───────────────────────────────────────────────────────────────────

/// The six corners of a flat-top hexagon (two points due east/west) of
/// circumradius `size`, centred at `center`, lying flat in the XZ plane. Corner
/// 0 is the west (+X) point; corners step 60° round. Edge `i` joins corner `i`
/// to corner `(i + 1) % 6`, and reads clockwise from above under the
/// `+X = west, +Z = north` convention.
pub fn hex_corners(center: Vec3, size: f32) -> [Vec3; 6] {
    let mut c = [Vec3::ZERO; 6];
    for (i, slot) in c.iter_mut().enumerate() {
        let a = i as f32 * std::f32::consts::FRAC_PI_3;
        *slot = center + Vec3::new(size * a.cos(), 0.0, size * a.sin());
    }
    c
}

/// Outward unit normal of edge `e` in the XZ plane — the direction from a hex
/// centre to its edge-`e` neighbour. Edge 0 (`a`) points northwest (+X/+Z); the
/// rest step round with the edges. Hex-independent, since every hex shares the
/// same orientation.
pub fn edge_normal(e: usize) -> Vec3 {
    let c = hex_corners(Vec3::ZERO, 1.0);
    let mid = (c[e] + c[(e + 1) % 6]) * 0.5;
    Vec3::new(mid.x, 0.0, mid.z).normalize_or_zero()
}

/// World centre of the hex reached by walking `steps` (unit edge-steps) from the
/// origin — the sum of the step directions scaled to the hex spacing. Now used
/// only to evaluate the `HEX_STEPS`/`LEFT_MAP_STEPS` reference tables in tests.
#[allow(dead_code)]
pub fn hex_center(steps: &[usize]) -> Vec3 {
    steps
        .iter()
        .fold(Vec3::ZERO, |acc, &s| acc + edge_normal(s))
        * HEX_SPACING
}

/// Latitude (polar angle from the pole) of ring `k` on a `rings`-ring map: the
/// quarter-turn pole→equator split evenly, so ring k sits `k·(90°/rings)` from
/// the pole (`k = 0` the pole, the outermost ring on the equator). The map draws
/// flat; this is retained only for the per-tile celestial-direction math.
pub fn ring_dome_angle(k: usize, rings: usize) -> f32 {
    k as f32 * (std::f32::consts::FRAC_PI_2 / rings as f32)
}

/// Static placement of one hex, independent of roll: its map-ordering `number`,
/// the flat pre-transform centre handed to `draw_hex`, whether it is
/// record-flipped, and which map (`left` ⇒ the left map's transform). Built once
/// by [`build_hex_instances`] and shared by drawing and the mouse pick so both
/// see identical geometry.
#[derive(Copy, Clone)]
pub struct HexInst {
    pub number: u32,
    pub center: Vec3,
    pub flip: bool,
    pub left: bool,
    /// Flat grid centre in the map's own logical frame. Within one map, two
    /// tiles are neighbours iff their `logical` centres are `HEX_SPACING` apart —
    /// the source of truth for same-map adjacency (roll-independent).
    pub logical: Vec3,
}

/// Build the full hex list for both maps at `rings` rings — the single source of
/// truth for tile numbering and placement. Grown straight from the ring formulas
/// (which reproduce the hand-built `HEX_STEPS`/`LEFT_MAP_STEPS` tables exactly,
/// per the `ring_formula` test), so any ring count lays out consistently. The
/// right map numbers centre-outward (0, then `first_ring(1..rings)`); the left
/// map continues the count outer-ring-inward (`left_ring(rings..1)`, then its
/// centre), so a ring grows in the middle of the whole sequence.
pub fn build_hex_instances(rings: usize) -> Vec<HexInst> {
    let mut v: Vec<HexInst> = Vec::with_capacity(2 * (1 + 3 * rings * (rings + 1)));
    let push = |v: &mut Vec<HexInst>, logical, center, flip, left| {
        let number = v.len() as u32;
        v.push(HexInst { number, center, flip, left, logical });
    };

    // Right map: centre, then each ring's spiral, outward — laid flat on the
    // ground (the drawn centre is the logical position itself).
    push(&mut v, Vec3::ZERO, Vec3::ZERO, false, false);
    for k in 1..=rings {
        for off in first_ring(k) {
            push(&mut v, off, off, false, false);
        }
    }

    // Left map: outer ring inward, then its centre. Record-flipped about the
    // south map's own centre column `cx = sep(rings)` — each tile mirrors west of
    // it and the centre lands exactly on `cx` (its roll column), so the map sits
    // clear, screen-left, however many rings.
    let cx = sep(rings);
    let c = left_center();
    let reflect = |p: Vec3| Vec3::new(cx + c.x - p.x, p.y, p.z);
    for k in (1..=rings).rev() {
        for off in left_ring(k) {
            push(&mut v, c + off, reflect(c + off), true, true);
        }
    }
    push(&mut v, c, reflect(c), true, true);
    v
}

/// Same-map neighbours of every tile, indexed by tile number: the (≤6) tiles in
/// the *same* map whose logical centre is one `HEX_SPACING` step away. Interior
/// tiles get all six here; equator (ring-3) tiles come up short — their missing
/// neighbours are filled across the equator at pick time (see
/// `HexScene::compute_highlight`). Static, so built once.
pub fn build_within_neighbors(hexes: &[HexInst]) -> Vec<Vec<u32>> {
    let tol = HEX_SPACING * 0.1; // logical neighbours are exactly one step apart
    hexes
        .iter()
        .map(|a| {
            hexes
                .iter()
                .filter(|b| {
                    b.number != a.number
                        && b.left == a.left
                        && ((b.logical - a.logical).length() - HEX_SPACING).abs() < tol
                })
                .map(|b| b.number)
                .collect()
        })
        .collect()
}

/// World-space corners of a hex after the per-tile `tilt` (fence stand-up;
/// identity when flat) and map `xform` — the same placement `draw_hex` draws,
/// used to build the pick triangles.
pub fn hex_world_corners(center: Vec3, flip: bool, tilt: Mat4, xform: &Mat4) -> [Vec3; 6] {
    let mut corners = hex_corners(center, HEX_SIZE);
    if flip {
        for c in corners.iter_mut() {
            *c = flip_ns(*c, center);
        }
    }
    corners.map(|p| xform.transform_point3(center + tilt.transform_vector3(p - center)))
}

/// Möller–Trumbore ray/triangle intersection, **double-sided** (no back-face
/// cull, so the down-facing left dome's tiles pick too). Returns the ray
/// parameter `t > 0` at the hit, else `None`.
pub fn ray_triangle(origin: Vec3, dir: Vec3, a: Vec3, b: Vec3, c: Vec3) -> Option<f32> {
    let e1 = b - a;
    let e2 = c - a;
    let h = dir.cross(e2);
    let det = e1.dot(h);
    if det.abs() < 1e-7 {
        return None; // ray parallel to the triangle
    }
    let inv = 1.0 / det;
    let s = origin - a;
    let u = inv * s.dot(h);
    if !(0.0..=1.0).contains(&u) {
        return None;
    }
    let q = s.cross(e1);
    let vbc = inv * dir.dot(q);
    if vbc < 0.0 || u + vbc > 1.0 {
        return None;
    }
    let t = inv * e2.dot(q);
    (t > 1e-4).then_some(t)
}

/// A unit translucent hexagon face for `Renderer::draw_mesh`: a centre vertex
/// plus six corners at `HEX_SIZE` in the local XZ plane, fanned into six
/// triangles — **double-sided** (a +Y front fan and a −Y back fan with reversed
/// winding) so the face shows whether viewed from above or below, since the mesh
/// pipeline back-face-culls. Per-tile placement comes from the model matrix.
pub fn build_hex_fill_mesh() -> (Vec<MeshVertex>, Vec<u32>) {
    let corner = |i: usize| -> [f32; 3] {
        let a = i as f32 * std::f32::consts::FRAC_PI_3;
        [HEX_SIZE * a.cos(), 0.0, HEX_SIZE * a.sin()]
    };
    let mut verts = Vec::with_capacity(14);
    // Front side (normal +Y): centre then 6 corners → indices 0..=6.
    verts.push(MeshVertex { position: [0.0; 3], normal: [0.0, 1.0, 0.0], material: FILL_MATERIAL });
    for i in 0..6 {
        verts.push(MeshVertex { position: corner(i), normal: [0.0, 1.0, 0.0], material: FILL_MATERIAL });
    }
    // Back side (normal −Y): centre then 6 corners → indices 7..=13.
    verts.push(MeshVertex { position: [0.0; 3], normal: [0.0, -1.0, 0.0], material: FILL_MATERIAL });
    for i in 0..6 {
        verts.push(MeshVertex { position: corner(i), normal: [0.0, -1.0, 0.0], material: FILL_MATERIAL });
    }
    let mut idx = Vec::with_capacity(36);
    for i in 0..6u32 {
        let j = (i + 1) % 6;
        idx.extend_from_slice(&[0, 1 + i, 1 + j]); // front fan
        idx.extend_from_slice(&[7, 8 + j, 8 + i]); // back fan (reversed winding)
    }
    (verts, idx)
}

/// Ring-`k` cell offsets from a centre (× `HEX_SPACING`): the classic clockwise
/// walk starting at the NW corner (`k·a`), `k` steps along each of the six sides
/// (side `s` runs in direction `s + 2`). This is the formula that lets us add a
/// ring to either map consistently — the hand-built tables above are exactly
/// rotations of these (see the `ring_formula` test).
pub fn ring_offsets(k: usize) -> Vec<Vec3> {
    let mut cells = Vec::with_capacity(6 * k);
    let mut cell = edge_normal(0) * (k as f32) * HEX_SPACING;
    for side in 0..6 {
        let step = edge_normal((side + 2) % 6) * HEX_SPACING;
        for _ in 0..k {
            cells.push(cell);
            cell += step;
        }
    }
    cells
}

/// The **right** map's ring `k`, in outward-spiral order: it enters one NW step
/// out from the previous ring's SW corner and **ends on its own SW corner**
/// (`k·f` rotated to last). Rotating by `5k+1` lands the SW corner last; this
/// reproduces the hand-built ring 1 (`a…f`) and ring 2 (`f+a…2f`) exactly.
pub fn first_ring(k: usize) -> Vec<Vec3> {
    let mut r = ring_offsets(k);
    r.rotate_left((5 * k + 1) % (6 * k));
    r
}

/// The **left** map's ring `k` (offsets from its centre), **starting on the SW
/// corner** (`k·f` first) — the hand-built left order, extended. Rotating by
/// `5k` lands the SW corner first.
pub fn left_ring(k: usize) -> Vec<Vec3> {
    let mut r = ring_offsets(k);
    r.rotate_left(5 * k);
    r
}

/// Logical centre of the left map (first-map coordinates, before the reflection)
/// — equal to the hand-built centre tile's position.
pub fn left_center() -> Vec3 {
    edge_normal(0) * HEX_SPACING
}

/// Mirror point `p` about the north–south (Z) axis through `center` — the
/// horizontal half of the left chart's record-flip: west↔east (`x → 2·cx − x`),
/// north/south unchanged. (The flip's Y-inversion is shown by the gadget, not
/// the tiles.)
pub fn flip_ns(p: Vec3, center: Vec3) -> Vec3 {
    Vec3::new(2.0 * center.x - p.x, p.y, p.z)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_geometry() {
        let c = hex_corners(Vec3::ZERO, HEX_SIZE);
        // Corner 0 is the west (+X) point; corner 3 the east (−X) point — 2048 apart.
        assert!((c[0] - Vec3::new(HEX_SIZE, 0.0, 0.0)).length() < 1e-3);
        assert!((c[3] - Vec3::new(-HEX_SIZE, 0.0, 0.0)).length() < 1e-3);
        assert!((c[0].x - c[3].x).abs() - HEX_WIDTH < 1e-3);
        // All corners lie on the ground plane at the circumradius.
        for corner in c {
            assert!(corner.y.abs() < 1e-3);
            assert!((corner.length() - HEX_SIZE).abs() < 1e-3);
        }
    }

    #[test]
    fn ring_neighbours() {
        // Neighbour k sits across edge k-1; centres = edge_normal(i) * spacing.
        let centers: Vec<Vec3> = (0..6).map(|i| edge_normal(i) * HEX_SPACING).collect();

        // Each is HEX_SPACING from the origin, on the ground, and leaves exactly
        // HEX_GAP between nearest edges (centre distance − two apothems).
        for c in &centers {
            assert!((c.length() - HEX_SPACING).abs() < 1e-2);
            assert!(c.y.abs() < 1e-3);
            assert!((c.length() - 2.0 * APOTHEM - HEX_GAP).abs() < 1e-2);
        }
        assert!(HEX_GAP > 0.0, "hexes are drawn with a visible gap");

        // The clockwise compass ring: 1 NW, 2 N, 3 NE, 4 SE, 5 S, 6 SW, under
        // +X = west, +Z = north. (#2/#5 are due N/S; the rest are intercardinal.)
        assert!(centers[0].x > 0.0 && centers[0].z > 0.0); // 1 NW
        assert!(centers[1].x.abs() < 1.0 && centers[1].z > 0.0); // 2 N
        assert!(centers[2].x < 0.0 && centers[2].z > 0.0); // 3 NE
        assert!(centers[3].x < 0.0 && centers[3].z < 0.0); // 4 SE
        assert!(centers[4].x.abs() < 1.0 && centers[4].z < 0.0); // 5 S
        assert!(centers[5].x > 0.0 && centers[5].z < 0.0); // 6 SW
    }

    #[test]
    fn ring2_spiral() {
        let s = HEX_SPACING;
        let step_a = edge_normal(0) * s;
        let step_b = edge_normal(1) * s;

        // Hex 7 sits due west (between the SW and NW corners), on the ground.
        let h7 = hex_center(HEX_STEPS[7]);
        assert!(h7.x > 0.0 && h7.z.abs() < 1.0 && h7.y.abs() < 1e-3);

        // 7 attaches across hex 6's edge a (NW): 6.a ↔ 7.d.
        assert!((h7 - hex_center(HEX_STEPS[6]) - step_a).length() < 1.0);

        // Hex 8 is one north step from hex 7 (8.e ↔ 7.b)...
        let h8 = hex_center(HEX_STEPS[8]);
        assert!((h8 - h7 - step_b).length() < 1.0);
        // ...and the ring closes: hex 18 is one north step *below* hex 7.
        let h18 = hex_center(HEX_STEPS[18]);
        assert!((h7 - h18 - step_b).length() < 1.0);

        // All 12 ring-2 hexes are distinct and farther out than ring 1.
        let r1 = HEX_SPACING;
        for n in 7..=18 {
            assert!(hex_center(HEX_STEPS[n]).length() > r1 + 1.0);
        }
    }

    #[test]
    fn ring_formula() {
        let s = HEX_SPACING;

        // The formula reproduces the hand-built right map *exactly*, so adding a
        // ring can't reorder the existing tiles: ring 1 == HEX_STEPS[1..=6],
        // ring 2 == HEX_STEPS[7..=18].
        let r1 = first_ring(1);
        for i in 0..6 {
            assert!((r1[i] - hex_center(HEX_STEPS[1 + i])).length() < 1e-2, "ring1[{i}]");
        }
        let r2 = first_ring(2);
        for i in 0..12 {
            assert!((r2[i] - hex_center(HEX_STEPS[7 + i])).length() < 1e-2, "ring2[{i}]");
        }

        // Ring 3 continues the spiral and ENDS ON A CORNER.
        let r3 = first_ring(3);
        assert_eq!(r3.len(), 18);
        // first cell = previous SW corner (HEX_STEPS[18] = 2f) + one NW step.
        assert!((r3[0] - (hex_center(HEX_STEPS[18]) + edge_normal(0) * s)).length() < 1e-2);
        // last cell = the ring-3 SW corner: radius 3·s, in the SW direction
        // (+X west, −Z south).
        let last = *r3.last().unwrap();
        assert!((last.length() / s - 3.0).abs() < 1e-2);
        assert!(last.x > 0.0 && last.z < 0.0);

        // The left ring formula reproduces the hand-built left ring 2 (offsets
        // from the left centre), and its ring 3 *starts* on the SW corner.
        let c = left_center();
        let lr2 = left_ring(2);
        for i in 0..12 {
            assert!((lr2[i] - (hex_center(LEFT_MAP_STEPS[i]) - c)).length() < 1e-2, "left2[{i}]");
        }
        let lr3 = left_ring(3);
        assert!((lr3[0].length() / s - 3.0).abs() < 1e-2 && lr3[0].x > 0.0 && lr3[0].z < 0.0);
    }

    #[test]
    fn left_map_column() {
        let s = HEX_SPACING;
        let p = |i: usize| hex_center(LEFT_MAP_STEPS[i]); // logical (pre-shift) positions

        // 19/20/21 form a vertical column, each one north step above the last.
        let step_b = edge_normal(1) * s;
        assert!((p(1) - p(0) - step_b).length() < 1.0);
        assert!((p(2) - p(1) - step_b).length() < 1.0);

        // Stated links place each tile against the right first-map hex:
        let step_c = edge_normal(2) * s; // NE
        let step_d = edge_normal(3) * s; // SE
        let at = |n: usize| hex_center(HEX_STEPS[n]);
        assert!((at(7) - (p(0) + step_c)).length() < 1.0); // 19.c ↔ 7.f  (7 NE of 19)
        assert!((at(18) - (p(0) + step_d)).length() < 1.0); // 19.d ↔ 18.a (18 SE of 19)
        assert!((at(8) - (p(1) + step_c)).length() < 1.0); // 20.c ↔ 8.f
        assert!((at(7) - (p(1) + step_d)).length() < 1.0); // 20.d ↔ 7.a
        assert!((at(8) - (p(2) + step_d)).length() < 1.0); // 21.d ↔ 8.a (corner: one inner edge)

        // 22/23 curve up — each new tile's f edge meets the prev c.
        assert!((p(3) - p(2) - step_c).length() < 1.0); // 22.f ↔ 21.c
        assert!((p(4) - p(3) - step_c).length() < 1.0); // 23.f ↔ 22.c
        // 24/25 over the top (a onto prev d → SE); 26/27 down the east edge
        // (b onto prev e → S).
        assert!((p(5) - p(4) - step_d).length() < 1.0); // 24.a ↔ 23.d
        assert!((p(6) - p(5) - step_d).length() < 1.0); // 25.a ↔ 24.d
        let step_s = edge_normal(4) * s;
        assert!((p(7) - p(6) - step_s).length() < 1.0); // 26.b ↔ 25.e
        assert!((p(8) - p(7) - step_s).length() < 1.0); // 27.b ↔ 26.e
        // 28/29 close the south (c onto prev f → SW); 30 turns NW onto 29 and
        // shuts the ring (30.d ↔ 29.a, then 30.a ↔ 19.d).
        let step_sw = edge_normal(5) * s;
        let step_nw = edge_normal(0) * s;
        assert!((p(9) - p(8) - step_sw).length() < 1.0); // 28.c ↔ 27.f
        assert!((p(10) - p(9) - step_sw).length() < 1.0); // 29.c ↔ 28.f
        assert!((p(11) - p(10) - step_nw).length() < 1.0); // 30.d ↔ 29.a
        assert!((p(0) - p(11) - step_nw).length() < 1.0); // 30.a ↔ 19.d (ring closes)

        // Tiles 19–30 form a ring-2 hexagon around centre (√3/2, ½)·s: corners
        // at radius 2·s, edge-hexes at √3·s.
        let centre = Vec3::new(0.866_025_4 * s, 0.0, 0.5 * s);
        for i in 0..12 {
            let r = (p(i) - centre).length() / s;
            assert!(r > 1.7 && r < 2.01, "tile {} radius {r}", i + 19);
        }

        // Inner ring 31–36 (marches rotate N,N,NE,SE,S,SW) closes onto 31, and
        // 37 is the centre tile.
        assert!((p(12) - p(11) - step_b).length() < 1.0); // 31.e ↔ 30.b
        assert!((p(13) - p(12) - step_b).length() < 1.0); // 32.e ↔ 31.b
        assert!((p(14) - p(13) - step_c).length() < 1.0); // 33.f ↔ 32.c
        assert!((p(15) - p(14) - step_d).length() < 1.0); // 34.a ↔ 33.d
        assert!((p(16) - p(15) - step_s).length() < 1.0); // 35.b ↔ 34.e
        assert!((p(17) - p(16) - step_sw).length() < 1.0); // 36.c ↔ 35.f
        assert!((p(12) - p(17) - step_nw).length() < 1.0); // 31 = 36 + NW (closes)
        assert!((p(18) - centre).length() < 1e-3); // 37 = centre tile
        // The 6 inner-ring tiles all sit one step from the centre.
        for i in 12..18 {
            assert!(((p(i) - centre).length() / s - 1.0).abs() < 1e-3, "tile {}", i + 19);
        }

        // The chart is drawn reflected to the west (screen-left), and the
        // separation grows with the ring count so larger maps stay clear.
        assert!(sep(1) > 0.0 && sep(2) > sep(1) && sep(3) > sep(2));
    }

    #[test]
    fn record_flip() {
        let center = Vec3::ZERO;
        let c = hex_corners(center, HEX_SIZE);
        let f: Vec<Vec3> = c.iter().map(|&p| flip_ns(p, center)).collect();
        let mid = |i: usize, src: &[Vec3]| (src[i] + src[(i + 1) % 6]) * 0.5;

        // b (edge 1) stays north (+Z), e (edge 4) stays south — the flip axis.
        assert!(mid(1, &f).z > 0.0 && mid(1, &f).x.abs() < 1.0);
        assert!(mid(4, &f).z < 0.0 && mid(4, &f).x.abs() < 1.0);
        // a (edge 0) was on the west (+X); after the flip it's on the east (−X).
        assert!(mid(0, &c).x > 0.0 && mid(0, &f).x < 0.0);
        // c (edge 2) was east; after the flip it's west — a/f and c/d swap sides.
        assert!(mid(2, &c).x < 0.0 && mid(2, &f).x > 0.0);
        // North/south components are untouched by the flip.
        for i in 0..6 {
            assert!((mid(i, &c).z - mid(i, &f).z).abs() < 1e-3);
        }
    }
}
