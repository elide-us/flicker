//! hex-map: the first brick of the world hex-map client. It establishes the
//! coordinate frame the map ordering will be built on top of, by drawing:
//!
//!   * an **XYZ compass** through the origin — red X, green Y, blue Z, with the
//!     world convention `+X = west, +Y = up, +Z = north` (right-handed, Y-up,
//!     matching voxel-cluster); each axis's positive half is bright with a
//!     pyramid arrowhead, the negative half dim, so `+`/`-` reads at a glance;
//!   * **flat-top hexagons**, 2048 units corner-to-corner (the two points face
//!     east/west: +X = west, −X = east), drawn as rasterized line wireframes —
//!     each hex lifted onto the cardinal dome (see below) and tilted tangent to
//!     it (its face normal along the radius from the dome centre), so it lies
//!     flat at the apex and stands vertical at the equator — with a **dot at
//!     each corner**, each edge in its own colour and labelled `a`–`f`
//!     (clockwise from the west point);
//!   * a **number billboard** over each hex centre — the per-hex map-ordering
//!     label. The right map is numbered centre-outward as one clockwise spiral:
//!     0 centre, ring 1 (1–6), ring 2 (7–18, hand-built), ring 3 (19–36, grown
//!     by the [`first_ring`] formula). Each ring ends on its SW corner, so the
//!     map ends on a corner (36 = 3f). Bump the ring count and the spiral grows
//!     in place.
//!   * a **second "left map"** to the west (screen-left), **record-flipped**
//!     about the N-S axis (Y down, west↔east mirrored; b/e stay N/S), numbered
//!     outer-ring-inward and continuing the count (37–73) so a new ring lands in
//!     the middle of the whole sequence; a second compass sits on its centre.
//!   * a **roll wheel** south of each map: left-drag to roll it about its N-S
//!     axis, 0–180° (tops always tilt away from each other).
//!   * a **cardinal dome** over each map: a hemisphere of radius `3·s` whose
//!     quarter-turn (apex → equator) is divided evenly across the rings, so ring
//!     k rests `k·(90°/3)` down the dome — rings 1/2/3 at 30°/60°/90°. Each ring
//!     is a **latitude circle** at that angle (horizontal radius `3s·sin θ`,
//!     lifted `3s·cos θ`) with its hexes — corner and straight-run alike —
//!     snapped onto it; two **meridian arcs** over the X (E–W) and Z (N–S)
//!     verticals run from the base equator (the outer ring, e.g. the right map's
//!     21↔30 span) up over the apex, where the centre tile sits. All lifted in
//!     +Y and rolled with the map.
//!   * **world-gen terrain** on every tile: the hex-world six-epoch stack
//!     (`flicker-worldgen`) runs over this layout's numbering and adjacency, and
//!     each tile draws the consolidated top layer — the Epoch-6 eroded ground
//!     with its sea — in place of the grey fill, while **all six epoch layers
//!     stay resident** for the ongoing generation tweaks (see `terrain.rs`).
//!     The ring slider rebuilds the whole world.
//!
//! It follows the voxel-cluster application model end to end: a [`Scene`] driven
//! by [`SceneManager`]/[`run`], the [`InputMap`]/[`AbstractControls`] input
//! system, and a Lua-scripted HUD (`scripts/hud.lua` + `ui_elements.json`)
//! published through the engine `Model`.
//!
//! Camera (MMO-style):
//!   * WASD — move forward/back and strafe in the camera's facing.
//!   * R / F — rise / descend (world up / down).
//!   * Right-drag — free-look yaw + pitch.
//!   * Escape — quit.

use std::time::Duration;

use anyhow::Result;
use flicker::app::{run, AbstractControls, Action, InputMap, InputState, Key};
use flicker::render::{
    Camera, Mat4, MeshDrawOptions, MeshHandle, MeshIndices, MeshVertex, Renderer, SceneLighting,
    TextureHandle, Vec2, Vec3,
};
use flicker::scene::{Scene, SceneManager, Transition};
use flicker::script::{ScriptHost, ValueMap};
use flicker::ui::{load_ui_json, load_widgets, render_hud};
use flicker_materials::Tables;

mod terrain;

const HUD_SCRIPT_PATH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/scripts/hud.lua");
const UI_ELEMENTS_PATH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/ui_elements.json");

/// Corner-to-corner width of the hex (east–west, point through point).
const HEX_WIDTH: f32 = 2048.0;
/// Circumradius = half the corner-to-corner width (centre → any corner).
const HEX_SIZE: f32 = HEX_WIDTH * 0.5;

/// Half-length of each compass axis. A touch past the hex so the arrowheads
/// clear its outline.
const AXIS_LEN: f32 = 1536.0;
/// World size of the arrowhead at each axis's positive tip.
const ARROW: f32 = 96.0;

/// World diameter of the dot billboard at each hex corner.
const DOT_SIZE: f32 = 56.0;
/// World height of the centre label's glyphs.
const LABEL_SIZE: f32 = 360.0;

/// Per-edge colours, a–f. Each of the six edges gets a distinct hue so the
/// map-ordering work can name exactly which edge connects to which.
const EDGE_COLORS: [[f32; 4]; 6] = [
    [0.95, 0.26, 0.26, 1.0], // a — red
    [0.98, 0.62, 0.20, 1.0], // b — orange
    [0.96, 0.90, 0.32, 1.0], // c — yellow
    [0.42, 0.86, 0.46, 1.0], // d — green
    [0.36, 0.68, 0.98, 1.0], // e — blue
    [0.80, 0.50, 0.96, 1.0], // f — violet
];
/// World height of each edge's a–f letter glyph.
const EDGE_LABEL_SIZE: f32 = 220.0;
/// How far past the edge midpoint (outward from the centre) the letter sits.
const EDGE_LABEL_OUT: f32 = 150.0;

/// Centre → edge-midpoint distance (apothem) = circumradius · √3/2.
const APOTHEM: f32 = HEX_SIZE * 0.866_025_4;
/// Empty space left between two neighbouring hexes' nearest edges.
const HEX_GAP: f32 = 384.0;
/// Centre-to-centre distance for edge-adjacent hexes, including the gap.
const HEX_SPACING: f32 = 2.0 * APOTHEM + HEX_GAP;

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
const HEX_STEPS: [&[usize]; 19] = [
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
/// **record-flipped** about [`LEFT_AXIS_X`] (logical X reflects, and each tile
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
const LEFT_MAP_STEPS: [&[usize]; 19] = [
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

/// X of the vertical axis the left chart is record-flipped about. Logical
/// positions reflect through it (`x → 2·axis − x`) to land west (screen-left)
/// of the first map; combined with each tile's mirror that's a true reflection.
/// Wide enough that the two (now ring-3) maps clear each other.
const LEFT_AXIS_X: f32 = 4.5 * HEX_SPACING;

/// Half-hex yaw of the left (down-facing) map about its own vertical centre
/// axis, given `rings`: half the outer ring's hex spacing. The outer ring has
/// `6·rings` tiles (`360°/6·rings` apart), so this is `−½` of that — negative
/// (CW about +Y), setting the lower ring's points into the upper ring's valleys.
/// (`rings = 3` → `−10°`, matching the earlier hand-tuned value.)
fn left_yaw(rings: usize) -> f32 {
    -std::f32::consts::PI / (6.0 * rings as f32)
}

/// Radius of the roll wheel handles.
const WHEEL_RADIUS: f32 = 850.0;
/// World Z (south) of each map's roll wheel, on the map's N-S axis — south of
/// the ring-3 extent.
const RIGHT_WHEEL_Z: f32 = -8000.0;
const LEFT_WHEEL_Z: f32 = -7000.0;
/// Screen-pixel radius for hit-testing a wheel against the cursor.
const WHEEL_PICK_PX: f32 = 90.0;
/// Roll change per pixel of vertical drag (radians).
const ROLL_SENS: f32 = 0.005;

/// Height the ring guide circles float above the (flat) tile plane, in +Y,
/// before the map's roll tilts them with it. Tall enough to clear the centre
/// number labels.
const GUIDE_LIFT: f32 = 700.0;
/// Colour of the ring guide circles (a soft cyan, distinct from the edge hues
/// and the yellow roll wheels).
const GUIDE_COLOR: [f32; 4] = [0.55, 0.85, 1.0, 0.9];
/// Line segments per guide circle.
const GUIDE_SEGS: usize = 96;

/// Ring-count range the slider spans, and the start value. Each map has a centre
/// tile plus `rings` rings; the dome's quarter-turn (apex → equator) is divided
/// evenly across `rings`, so ring `k` rests `k·(90°/rings)` down the dome.
const MIN_RINGS: usize = 1;
const MAX_RINGS: usize = 5;
const DEFAULT_RINGS: usize = 3;

/// Dome (hemisphere) radius for `rings`: the outermost ring sits on its equator,
/// so the radius is `rings · HEX_SPACING`.
fn dome_radius(rings: usize) -> f32 {
    rings as f32 * HEX_SPACING
}

// Ring-count slider (screen-space, pixels from the top-left, below the HUD
// stats block). Drag the handle 1‥5 to grow/shrink both maps.
const SLIDER_X0: f32 = 20.0;
const SLIDER_X1: f32 = 220.0;
const SLIDER_Y: f32 = 210.0;
const SLIDER_TRACK_H: f32 = 6.0;
const SLIDER_HANDLE_W: f32 = 14.0;
const SLIDER_HANDLE_H: f32 = 28.0;
/// Cursor distance (px) within which a press grabs the slider.
const SLIDER_HIT_PAD: f32 = 22.0;

/// World seed driving the six-epoch world-gen stack (hex-world's test seed).
const WORLD_SEED: u64 = 0x0EC0_DE01;
/// Tint (× the lit base colour) of a hovered/selected tile's terrain — stands
/// in for the grey fill's `HOVER_TINT` once terrain is drawn.
const TERRAIN_HOVER_TINT: [f32; 4] = [1.45, 1.45, 1.45, 1.0];

/// Material index of the hex face fill — CLOUD_MID, a neutral mid-grey in the
/// mesh palette (so the fill reads grey, not coloured).
const FILL_MATERIAL: u32 = 7;
/// Tint (× the lit base colour) of a resting tile fill; the `.w` is its opacity
/// — 25% transparent grey.
const FILL_TINT: [f32; 4] = [1.0, 1.0, 1.0, 0.25];
/// Tint of the hovered tile fill — brighter and more opaque, to read as a
/// highlight alongside the outward stretch.
const HOVER_TINT: [f32; 4] = [1.6, 1.6, 1.6, 0.55];
/// How far a hovered tile extends outward (along its radial normal) from its
/// resting spot on the dome — the hover "stretch" highlight.
const HOVER_STRETCH: f32 = 700.0;
/// Tiny inward (−radial) offset of the fill below the wireframe plane, so the
/// coloured edges always draw in front of the fill (no z-fighting).
const FILL_INSET: f32 = 4.0;
/// Hover hit-test rate — the mouse pick runs this many times a second, not every
/// frame.
const PICK_HZ: f32 = 15.0;
/// Vertical FOV used to build the pick ray; matches the render camera.
const PICK_FOV_Y: f32 = std::f32::consts::PI / 3.0; // 60°

// ───────────────────────────────────────────────────────────────────
// Geometry helpers
// ───────────────────────────────────────────────────────────────────

/// The six corners of a flat-top hexagon (two points due east/west) of
/// circumradius `size`, centred at `center`, lying flat in the XZ plane. Corner
/// 0 is the west (+X) point; corners step 60° round. Edge `i` joins corner `i`
/// to corner `(i + 1) % 6`, and reads clockwise from above under the
/// `+X = west, +Z = north` convention.
fn hex_corners(center: Vec3, size: f32) -> [Vec3; 6] {
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
fn edge_normal(e: usize) -> Vec3 {
    let c = hex_corners(Vec3::ZERO, 1.0);
    let mid = (c[e] + c[(e + 1) % 6]) * 0.5;
    Vec3::new(mid.x, 0.0, mid.z).normalize_or_zero()
}

/// World centre of the hex reached by walking `steps` (unit edge-steps) from the
/// origin — the sum of the step directions scaled to the hex spacing. Now used
/// only to evaluate the `HEX_STEPS`/`LEFT_MAP_STEPS` reference tables in tests.
#[allow(dead_code)]
fn hex_center(steps: &[usize]) -> Vec3 {
    steps
        .iter()
        .fold(Vec3::ZERO, |acc, &s| acc + edge_normal(s))
        * HEX_SPACING
}

/// Polar angle (from the apex) at which ring `k` rests on a `rings`-ring dome:
/// the quarter-turn (apex → equator) split evenly, so ring k sits `k·(90°/rings)`
/// down from the apex (the outermost on the base equator; `k = 0` is the apex).
fn ring_dome_angle(k: usize, rings: usize) -> f32 {
    k as f32 * (std::f32::consts::FRAC_PI_2 / rings as f32)
}

/// Horizontal radius of ring `k`'s circle on a `rings`-ring dome — `radius · sin θ`
/// at the ring's dome angle θ. (0 at the apex, full radius at the base equator.)
fn ring_dome_radius(k: usize, rings: usize) -> f32 {
    dome_radius(rings) * ring_dome_angle(k, rings).sin()
}

/// World height of ring `k`'s circle above the map centre — `GUIDE_LIFT +
/// radius · cos θ`. (`GUIDE_LIFT + radius` at the apex, `GUIDE_LIFT` at the base.)
fn ring_dome_lift(k: usize, rings: usize) -> f32 {
    GUIDE_LIFT + dome_radius(rings) * ring_dome_angle(k, rings).cos()
}

/// Place a tile from its `offset` from the map centre onto a `rings`-ring dome:
/// read its ring `k` from the offset's length, then set it on that ring's dome
/// circle, **evenly distributed** — its azimuth is snapped to the nearest of the
/// `6k` slots evenly spaced around the ring (anchored on the corners at 30°+60°s),
/// at horizontal radius [`ring_dome_radius`] and lifted [`ring_dome_lift`]. The
/// projection (the raw offset direction) only fixed the *ordering*; its angular
/// wobble between corners is regularised away here, so each ring is uniform. The
/// centre tile rides to the apex. Only the location moves — the hex keeps shape;
/// `logical` (same-map adjacency) is unaffected, so the topology is unchanged.
fn dome_position(offset: Vec3, rings: usize) -> Vec3 {
    let r = offset.length();
    if r < 1e-3 {
        return Vec3::new(0.0, ring_dome_lift(0, rings), 0.0);
    }
    let k = (r / HEX_SPACING).round() as usize;
    // Snap the azimuth to ring k's even grid (slot = 2π/6k, anchored at 30°).
    let slot = std::f32::consts::PI / (3.0 * k as f32);
    let base = std::f32::consts::FRAC_PI_6;
    let theta = base + ((offset.z.atan2(offset.x) - base) / slot).round() * slot;
    let dir = Vec3::new(theta.cos(), 0.0, theta.sin());
    dir * ring_dome_radius(k, rings) + Vec3::new(0.0, ring_dome_lift(k, rings), 0.0)
}

/// Rotation that tilts a flat hex (whose normal is +Y) so its normal becomes the
/// unit vector `n`: the shortest-arc rotation taking +Y to `n`. Laying each hex
/// tangent to the dome — perpendicular to the radius from the dome centre — uses
/// this with `n` = that radius, so a tile rides flat at the apex (`n = +Y`) and
/// stands vertical at the equator (`n` horizontal).
fn tilt_to_normal(n: Vec3) -> Mat4 {
    let d = Vec3::Y.dot(n).clamp(-1.0, 1.0);
    if d > 0.999_999 {
        Mat4::IDENTITY
    } else if d < -0.999_999 {
        Mat4::from_axis_angle(Vec3::X, std::f32::consts::PI)
    } else {
        Mat4::from_axis_angle(Vec3::Y.cross(n).normalize(), d.acos())
    }
}

/// Static placement of one hex, independent of roll: its map-ordering `number`,
/// the pre-transform centre handed to [`HexScene::draw_hex`], whether it is
/// record-flipped, the dome centre its tilt is measured from, and which map
/// (`left` ⇒ the left map's transform). Built once by [`build_hex_instances`]
/// and shared by drawing and the mouse pick so both see identical geometry.
#[derive(Copy, Clone)]
struct HexInst {
    number: u32,
    center: Vec3,
    flip: bool,
    dome_center: Vec3,
    left: bool,
    /// Pre-dome grid centre in the map's own logical frame. Within one map, two
    /// tiles are neighbours iff their `logical` centres are `HEX_SPACING` apart —
    /// the source of truth for same-map adjacency (roll-independent).
    logical: Vec3,
}

/// The fold-in layout for a selected tile: its six neighbours placed flat in its
/// tangent plane, against its edges. `slots` pairs each neighbour's number with
/// its pre-transform centre in the selected tile's plane; `tilt`/`flip`/`left`
/// are the selected tile's orientation and map (so neighbours draw coplanar with
/// it). `center` is the selected tile's own number (the anchor).
struct Rosette {
    center: u32,
    left: bool,
    flip: bool,
    tilt: Mat4,
    slots: Vec<(u32, Vec3)>,
}

/// Build the full hex list for both maps at `rings` rings — the single source of
/// truth for tile numbering and placement. Grown straight from the ring formulas
/// (which reproduce the hand-built `HEX_STEPS`/`LEFT_MAP_STEPS` tables exactly,
/// per the `ring_formula` test), so any ring count lays out consistently. The
/// right map numbers centre-outward (0, then `first_ring(1..rings)`); the left
/// map continues the count outer-ring-inward (`left_ring(rings..1)`, then its
/// centre), so a ring grows in the middle of the whole sequence.
fn build_hex_instances(rings: usize) -> Vec<HexInst> {
    let mut v: Vec<HexInst> = Vec::with_capacity(2 * (1 + 3 * rings * (rings + 1)));
    let push = |v: &mut Vec<HexInst>, logical, center, flip, dome_center, left| {
        let number = v.len() as u32;
        v.push(HexInst { number, center, flip, dome_center, left, logical });
    };

    // Right map: centre, then each ring's spiral, outward.
    let right_dome = Vec3::new(0.0, GUIDE_LIFT, 0.0);
    push(&mut v, Vec3::ZERO, dome_position(Vec3::ZERO, rings), false, right_dome, false);
    for k in 1..=rings {
        for off in first_ring(k) {
            push(&mut v, off, dome_position(off, rings), false, right_dome, false);
        }
    }

    // Left map: outer ring inward, then its centre (record-flipped + reflected).
    let reflect = |p: Vec3| Vec3::new(2.0 * LEFT_AXIS_X - p.x, p.y, p.z);
    let c = left_center();
    let left_dome = reflect(c) + Vec3::new(0.0, GUIDE_LIFT, 0.0);
    for k in (1..=rings).rev() {
        for off in left_ring(k) {
            push(&mut v, c + off, reflect(c + dome_position(off, rings)), true, left_dome, true);
        }
    }
    push(&mut v, c, reflect(c + dome_position(Vec3::ZERO, rings)), true, left_dome, true);
    v
}

/// Same-map neighbours of every tile, indexed by tile number: the (≤6) tiles in
/// the *same* map whose logical centre is one `HEX_SPACING` step away. Interior
/// tiles get all six here; equator (ring-3) tiles come up short — their missing
/// neighbours are filled across the equator at pick time (see
/// [`HexScene::compute_highlight`]). Static, so built once.
fn build_within_neighbors(hexes: &[HexInst]) -> Vec<Vec<u32>> {
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

/// World-space corners of a hex after the tilt-tangent + map `xform` — the same
/// placement [`HexScene::draw_hex`] draws, used to build the pick triangles.
fn hex_world_corners(center: Vec3, flip: bool, dome_center: Vec3, xform: &Mat4) -> [Vec3; 6] {
    let mut corners = hex_corners(center, HEX_SIZE);
    if flip {
        for c in corners.iter_mut() {
            *c = flip_ns(*c, center);
        }
    }
    let tilt = tilt_to_normal((center - dome_center).normalize_or_zero());
    corners.map(|p| xform.transform_point3(center + tilt.transform_vector3(p - center)))
}

/// Möller–Trumbore ray/triangle intersection, **double-sided** (no back-face
/// cull, so the down-facing left dome's tiles pick too). Returns the ray
/// parameter `t > 0` at the hit, else `None`.
fn ray_triangle(origin: Vec3, dir: Vec3, a: Vec3, b: Vec3, c: Vec3) -> Option<f32> {
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

/// A unit translucent hexagon face for [`Renderer::draw_mesh`]: a centre vertex
/// plus six corners at `HEX_SIZE` in the local XZ plane, fanned into six
/// triangles — **double-sided** (a +Y front fan and a −Y back fan with reversed
/// winding) so the face shows whether viewed from above or below, since the mesh
/// pipeline back-face-culls. Per-tile placement comes from the model matrix.
fn build_hex_fill_mesh() -> (Vec<MeshVertex>, Vec<u32>) {
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
fn ring_offsets(k: usize) -> Vec<Vec3> {
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
fn first_ring(k: usize) -> Vec<Vec3> {
    let mut r = ring_offsets(k);
    r.rotate_left((5 * k + 1) % (6 * k));
    r
}

/// The **left** map's ring `k` (offsets from its centre), **starting on the SW
/// corner** (`k·f` first) — the hand-built left order, extended. Rotating by
/// `5k` lands the SW corner first.
fn left_ring(k: usize) -> Vec<Vec3> {
    let mut r = ring_offsets(k);
    r.rotate_left(5 * k);
    r
}

/// Logical centre of the left map (first-map coordinates, before the reflection)
/// — equal to the hand-built centre tile's position.
fn left_center() -> Vec3 {
    edge_normal(0) * HEX_SPACING
}

/// Mirror point `p` about the north–south (Z) axis through `center` — the
/// horizontal half of the left chart's record-flip: west↔east (`x → 2·cx − x`),
/// north/south unchanged. (The flip's Y-inversion is shown by the gadget, not
/// the flat tiles, which sit on `y = 0`.)
fn flip_ns(p: Vec3, center: Vec3) -> Vec3 {
    Vec3::new(2.0 * center.x - p.x, p.y, p.z)
}

/// Draw an XYZ compass gadget at `origin`: red X, green Y, blue Z, each positive
/// half bright with a pyramid arrowhead and the negative half dim. With `flip`
/// the frame is mirrored west↔east about the N-S axis — **X points east**, Y/up
/// and Z/north unchanged — matching the X-mirrored left-chart tiles.
fn draw_compass(renderer: &mut Renderer, origin: Vec3, flip: bool, xform: &Mat4) {
    let axes = [
        (Vec3::X, [0.95, 0.30, 0.30, 1.0]),
        (Vec3::Y, [0.40, 0.90, 0.45, 1.0]),
        (Vec3::Z, [0.40, 0.62, 1.00, 1.0]),
    ];
    for (dir, color) in axes {
        let d = if flip {
            Vec3::new(-dir.x, dir.y, dir.z)
        } else {
            dir
        };
        let dim = [color[0] * 0.4, color[1] * 0.4, color[2] * 0.4, 0.7];
        let tip = origin + d * AXIS_LEN;
        let t = |p: Vec3| xform.transform_point3(p);
        renderer.draw_lines(&[(t(origin), t(origin - d * AXIS_LEN))], dim);
        renderer.draw_lines(&[(t(origin), t(tip))], color);
        for (s, e) in arrowhead(tip, d, ARROW) {
            renderer.draw_lines(&[(t(s), t(e))], color);
        }
    }
}

/// World X of the left map's centre column (the reflected centre tile 37) — the
/// vertical line the left map rolls about.
fn left_axis_x() -> f32 {
    2.0 * LEFT_AXIS_X - left_center().x
}

/// Roll transform: rotate about the vertical Z-line at world X `axis_x` by
/// `angle`. The map's N-S axis stays fixed; its east/west halves tilt up/down.
fn roll_transform(axis_x: f32, angle: f32) -> Mat4 {
    Mat4::from_translation(Vec3::new(axis_x, 0.0, 0.0))
        * Mat4::from_rotation_z(angle)
        * Mat4::from_translation(Vec3::new(-axis_x, 0.0, 0.0))
}

/// Project a world point to screen pixels (origin top-left). `None` if behind
/// the camera. Used to hit-test the roll wheels against the cursor.
fn project_to_screen(world: Vec3, view_proj: Mat4, screen: Vec2) -> Option<Vec2> {
    let clip = view_proj * world.extend(1.0);
    if clip.w <= 1e-3 {
        return None;
    }
    let ndc = clip.truncate() / clip.w;
    Some(Vec2::new(
        (ndc.x * 0.5 + 0.5) * screen.x,
        (1.0 - (ndc.y * 0.5 + 0.5)) * screen.y,
    ))
}

/// Draw a roll wheel: a vertical ring (XY plane) with three spokes whose angle
/// tracks `roll`, so dragging it visibly turns. The wheel sits on the map's
/// Z-axis (the roll axis) and is *not* itself rolled.
fn draw_wheel(renderer: &mut Renderer, centre: Vec3, radius: f32, roll: f32, color: [f32; 4]) {
    use std::f32::consts::TAU;
    const SEGS: usize = 32;
    let rim: Vec<(Vec3, Vec3)> = (0..SEGS)
        .map(|i| {
            let a0 = i as f32 / SEGS as f32 * TAU;
            let a1 = (i + 1) as f32 / SEGS as f32 * TAU;
            (
                centre + Vec3::new(radius * a0.cos(), radius * a0.sin(), 0.0),
                centre + Vec3::new(radius * a1.cos(), radius * a1.sin(), 0.0),
            )
        })
        .collect();
    renderer.draw_lines(&rim, color);
    for k in 0..3 {
        let a = roll + k as f32 * TAU / 3.0;
        let tip = centre + Vec3::new(radius * a.cos(), radius * a.sin(), 0.0);
        renderer.draw_lines(&[(centre, tip)], color);
    }
}

/// Draw a circle (or partial arc) of `radius` about `center`, lying in the plane
/// spanned by unit vectors `u`,`v` (point = `center + radius·(cos·u + sin·v)`),
/// over the angle span `[a0, a1]` in `segs` segments, then tilted by the map's
/// roll `xform`. With `u = X, v = Z, [0, TAU]` it's a flat ring on the tile
/// plane; with `u = X|Z, v = Y, [0, PI]` it's a vertical dome half-arc rising
/// from the `±u` cardinal points to an apex at `center + radius·Y`.
#[allow(clippy::too_many_arguments)]
fn draw_arc(
    renderer: &mut Renderer,
    center: Vec3,
    radius: f32,
    u: Vec3,
    v: Vec3,
    a0: f32,
    a1: f32,
    segs: usize,
    xform: &Mat4,
    color: [f32; 4],
) {
    let p = |a: f32| xform.transform_point3(center + (u * a.cos() + v * a.sin()) * radius);
    let lines: Vec<(Vec3, Vec3)> = (0..segs)
        .map(|i| {
            let t0 = a0 + (a1 - a0) * i as f32 / segs as f32;
            let t1 = a0 + (a1 - a0) * (i + 1) as f32 / segs as f32;
            (p(t0), p(t1))
        })
        .collect();
    renderer.draw_lines(&lines, color);
}

/// Four short segments forming a pyramid arrowhead at `tip`, opening back along
/// `-dir`. `dir` must be unit length.
fn arrowhead(tip: Vec3, dir: Vec3, size: f32) -> [(Vec3, Vec3); 4] {
    // Any axis not parallel to `dir` seeds the perpendicular basis.
    let seed = if dir.x.abs() > 0.9 { Vec3::Y } else { Vec3::X };
    let p1 = dir.cross(seed).normalize_or_zero();
    let p2 = dir.cross(p1).normalize_or_zero();
    let base = tip - dir * size;
    [
        (tip, base + p1 * size * 0.5),
        (tip, base - p1 * size * 0.5),
        (tip, base + p2 * size * 0.5),
        (tip, base - p2 * size * 0.5),
    ]
}

// ───────────────────────────────────────────────────────────────────
// Dot + digit billboard textures
// ───────────────────────────────────────────────────────────────────

fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// A soft white disc on transparent black — billboarded at each hex corner.
fn build_disc_texture() -> Vec<u8> {
    const S: usize = 16;
    let mut px = vec![0u8; S * S * 4];
    let c = (S as f32 - 1.0) * 0.5;
    for y in 0..S {
        for x in 0..S {
            let dx = x as f32 - c;
            let dy = y as f32 - c;
            let d = (dx * dx + dy * dy).sqrt() / c; // 0 centre … 1 edge
            let a = 1.0 - smoothstep(0.65, 1.0, d); // soft round edge
            let i = (y * S + x) * 4;
            px[i] = 255;
            px[i + 1] = 255;
            px[i + 2] = 255;
            px[i + 3] = (a * 255.0) as u8;
        }
    }
    px
}

// Tiny 5×7 bitmap font baked into a 16-cell atlas, one cell per character in
// `CHARSET`. Digits label the hexes (today just "0"); the lowercase letters a–f
// label the six edges for the map-ordering work.
const GLYPH_W: usize = 5;
const GLYPH_H: usize = 7;
const CHARSET: &[u8] = b"0123456789abcdef";
const GLYPHS: [[u8; GLYPH_H]; 16] = [
    [0b01110, 0b10001, 0b10011, 0b10101, 0b11001, 0b10001, 0b01110], // 0
    [0b00100, 0b01100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110], // 1
    [0b01110, 0b10001, 0b00001, 0b00010, 0b00100, 0b01000, 0b11111], // 2
    [0b11111, 0b00010, 0b00100, 0b00010, 0b00001, 0b10001, 0b01110], // 3
    [0b00010, 0b00110, 0b01010, 0b10010, 0b11111, 0b00010, 0b00010], // 4
    [0b11111, 0b10000, 0b11110, 0b00001, 0b00001, 0b10001, 0b01110], // 5
    [0b00110, 0b01000, 0b10000, 0b11110, 0b10001, 0b10001, 0b01110], // 6
    [0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b01000, 0b01000], // 7
    [0b01110, 0b10001, 0b10001, 0b01110, 0b10001, 0b10001, 0b01110], // 8
    [0b01110, 0b10001, 0b10001, 0b01111, 0b00001, 0b00010, 0b01100], // 9
    [0b00000, 0b00000, 0b01110, 0b00001, 0b01111, 0b10001, 0b01111], // a
    [0b10000, 0b10000, 0b10000, 0b11110, 0b10001, 0b10001, 0b11110], // b
    [0b00000, 0b00000, 0b01110, 0b10001, 0b10000, 0b10001, 0b01110], // c
    [0b00001, 0b00001, 0b00001, 0b01111, 0b10001, 0b10001, 0b01111], // d
    [0b00000, 0b00000, 0b01110, 0b10001, 0b11111, 0b10000, 0b01110], // e
    [0b00110, 0b01001, 0b01000, 0b11110, 0b01000, 0b01000, 0b01000], // f
];

const CELL_W: usize = 32;
const CELL_H: usize = 40;
const GLYPH_COUNT: usize = GLYPHS.len();
const ATLAS_W: usize = CELL_W * GLYPH_COUNT;
const ATLAS_H: usize = CELL_H;

/// Build the RGBA8 glyph atlas: one scaled-up white glyph per `CHARSET` cell on
/// transparent black.
fn build_glyph_atlas() -> Vec<u8> {
    const SCALE: usize = 4;
    let glyph_px_w = GLYPH_W * SCALE;
    let glyph_px_h = GLYPH_H * SCALE;
    let margin_x = (CELL_W - glyph_px_w) / 2;
    let margin_y = (CELL_H - glyph_px_h) / 2;

    let mut pixels = vec![0u8; ATLAS_W * ATLAS_H * 4];
    for (cell, rows) in GLYPHS.iter().enumerate() {
        let cell_x0 = cell * CELL_W + margin_x;
        for (row_idx, &row_bits) in rows.iter().enumerate() {
            for col in 0..GLYPH_W {
                if (row_bits >> (GLYPH_W - 1 - col)) & 1 == 0 {
                    continue;
                }
                for dy in 0..SCALE {
                    for dx in 0..SCALE {
                        let px = cell_x0 + col * SCALE + dx;
                        let py = margin_y + row_idx * SCALE + dy;
                        let i = (py * ATLAS_W + px) * 4;
                        pixels[i] = 0xff;
                        pixels[i + 1] = 0xff;
                        pixels[i + 2] = 0xff;
                        pixels[i + 3] = 0xff;
                    }
                }
            }
        }
    }
    pixels
}

/// Atlas cell index for character `c`, if it is in `CHARSET`.
fn glyph_cell(c: char) -> Option<usize> {
    CHARSET.iter().position(|&b| b as char == c)
}

/// UV sub-rect of the atlas for cell `cell`.
fn glyph_uv(cell: usize) -> (Vec2, Vec2) {
    let w = 1.0 / GLYPH_COUNT as f32;
    let u0 = cell as f32 * w;
    (Vec2::new(u0, 0.0), Vec2::new(u0 + w, 1.0))
}

/// World-X of glyph `i` of an `n`-glyph label centred at world-X `cx`. Glyphs
/// run toward −X (east) so that under the `+X = west` convention — where
/// screen-left is +X — the first character sits at the greatest X (screen-left)
/// and the label reads left→right (so "10" is not painted as "01").
fn glyph_x(cx: f32, i: usize, n: usize, glyph: f32) -> f32 {
    cx + glyph * (n as f32 * 0.5 - 0.5 - i as f32)
}

// ───────────────────────────────────────────────────────────────────
// Scene
// ───────────────────────────────────────────────────────────────────

/// Which roll wheel the cursor is currently dragging.
#[derive(Copy, Clone, PartialEq)]
enum Wheel {
    Right,
    Left,
}

struct HexScene {
    /// Camera eye position (world).
    position: Vec3,
    /// Look angles, right-handed Y-up. yaw 0 faces +Z.
    yaw: f32,
    pitch: f32,
    /// Cursor at the previous look frame, for the right-drag delta.
    last_look_cursor: Option<Vec2>,
    bindings: InputMap,
    controls: AbstractControls,

    /// Per-map roll about the N-S axis, 0..π. Right map: 0 = upright, π = upside
    /// down (rolls left). Left map: 0 = down-facing (its record-flipped start),
    /// π = upright (rolls right). Both tilt their tops *away* from each other.
    right_roll: f32,
    left_roll: f32,
    /// The roll wheel being left-dragged, and the cursor at the previous frame.
    drag: Option<Wheel>,
    drag_cursor: Vec2,
    /// True while the ring-count slider handle is being dragged.
    slider_drag: bool,

    /// Number of rings per map (1‥5), set by the slider. Drives the tile layout,
    /// dome radius/divisions, and equator yaw.
    num_rings: usize,
    /// Per-hex placement for the current ring count (number, centre, flip, dome
    /// centre, which map), shared by drawing and the pick test. Rebuilt on every
    /// ring-count change; roll-independent otherwise.
    hexes: Vec<HexInst>,
    /// Same-map neighbours per tile number (rebuilt with `hexes`).
    within: Vec<Vec<u32>>,
    /// Number of the hex the cursor is hovering, if any (mouse-pick result).
    hovered: Option<u32>,
    /// The clicked/selected hex, if any (toggled by left-click). While set, its
    /// six neighbours fold flat into its tangent plane, aligned to its edges,
    /// instead of pushing outward.
    selected: Option<u32>,
    /// Tiles to stretch this frame: the hovered tile plus its six neighbours
    /// (same-map + across-equator). Recomputed with `hovered` at [`PICK_HZ`].
    highlight: Vec<u32>,
    /// Seconds since the last hover pick — the hit-test runs at [`PICK_HZ`].
    pick_accum: f32,

    // GPU handles, uploaded once in `enter`.
    white: Option<TextureHandle>,
    dot: Option<TextureHandle>,
    glyphs: Option<TextureHandle>,
    /// Unit translucent hexagon face, drawn per tile as the pickable surface.
    fill_mesh: Option<MeshHandle>,
    script: Option<ScriptHost>,

    /// Materials vocabulary the epochs run on; `None` if the JSON tables failed
    /// to load (tiles then keep the plain grey fill).
    tables: Option<Tables>,
    /// The generated world — all six epoch layers per tile, retained so the
    /// ongoing generation tweaks can re-read any layer. Drawing uses only the
    /// consolidated top layer.
    world: Option<terrain::WorldGen>,
    /// Per-tile ground + sea meshes (indexed by tile number).
    terrain: Vec<terrain::TileTerrain>,
    /// World/meshes need (re)building next frame — set at startup and by the
    /// ring slider; the upload needs the `&mut Renderer` only `render` holds.
    terrain_dirty: bool,
}

impl HexScene {
    fn new() -> Self {
        let hexes = build_hex_instances(DEFAULT_RINGS);
        let within = build_within_neighbors(&hexes);
        Self {
            // Above and south, angled down — framed for both ring-3 maps plus
            // the roll wheels to their south.
            position: Vec3::new(8800.0, 8600.0, -15500.0),
            yaw: 0.0,    // face +Z (north)
            pitch: -0.5, // look down at it
            last_look_cursor: None,
            bindings: InputMap::wasd_and_mouse(),
            controls: AbstractControls {
                // Two big charts side by side (~33 k across) — fly fast.
                move_speed: 4000.0,
                ..AbstractControls::default()
            },
            right_roll: 0.0,
            left_roll: std::f32::consts::PI, // rolled to π: left map faces directly down
            drag: None,
            drag_cursor: Vec2::ZERO,
            slider_drag: false,
            num_rings: DEFAULT_RINGS,
            hexes,
            within,
            hovered: None,
            selected: None,
            highlight: Vec::new(),
            pick_accum: 0.0,
            white: None,
            dot: None,
            glyphs: None,
            fill_mesh: None,
            script: None,
            tables: terrain::load_tables(),
            world: None,
            terrain: Vec::new(),
            terrain_dirty: true,
        }
    }

    /// Unit vector the camera looks along, from yaw/pitch. Right-handed Y-up.
    fn forward(&self) -> Vec3 {
        let cp = self.pitch.cos();
        Vec3::new(cp * self.yaw.sin(), self.pitch.sin(), cp * self.yaw.cos())
    }

    /// Horizontal forward (ignores pitch so WASD stays in the XZ plane).
    fn move_forward(&self) -> Vec3 {
        let f = self.forward();
        Vec3::new(f.x, 0.0, f.z).normalize_or_zero()
    }

    /// Horizontal "right" relative to facing (matches the renderer's view basis).
    fn move_right(&self) -> Vec3 {
        let flat = self.move_forward();
        flat.cross(Vec3::Y).normalize_or_zero()
    }

    /// The two maps' roll/placement transforms (right, left) for the given roll
    /// angles. The left map's chain is roll → drop+Z-align → half-hex yaw →
    /// slide under the right map (see the inline notes where each piece is
    /// defined). The terrain build calls this with the **rest pose** (right 0,
    /// left π) so the generated world is independent of the wheels.
    fn map_transforms_for(rings: usize, right_roll: f32, left_roll: f32) -> (Mat4, Mat4) {
        let lx = left_axis_x();
        let m_right = roll_transform(0.0, right_roll);
        let left_shift = Vec3::new(0.0, -HEX_SIZE, -0.5 * HEX_SPACING);
        // Half-hex yaw scales with the ring count (outer ring = 6·rings tiles).
        let yaw = Mat4::from_translation(Vec3::new(lx, 0.0, 0.0))
            * Mat4::from_rotation_y(left_yaw(rings))
            * Mat4::from_translation(Vec3::new(-lx, 0.0, 0.0));
        let left_under = Mat4::from_translation(Vec3::new(-lx, 0.0, 0.0));
        let m_left =
            left_under * yaw * Mat4::from_translation(left_shift) * roll_transform(lx, -left_roll);
        (m_right, m_left)
    }

    /// This frame's map transforms — shared by `render` (drawing) and `pick`
    /// (hit-test) so both agree.
    fn map_transforms(&self) -> (Mat4, Mat4) {
        Self::map_transforms_for(self.num_rings, self.right_roll, self.left_roll)
    }

    /// Set the ring count (clamped to `MIN_RINGS‥MAX_RINGS`) and, if it changed,
    /// rebuild the tile layout and neighbour table and clear the hover/selection
    /// (tile numbers change with the count).
    fn set_rings(&mut self, rings: usize) {
        let rings = rings.clamp(MIN_RINGS, MAX_RINGS);
        if rings == self.num_rings {
            return;
        }
        self.num_rings = rings;
        self.hexes = build_hex_instances(rings);
        self.within = build_within_neighbors(&self.hexes);
        self.hovered = None;
        self.selected = None;
        self.highlight.clear();
        // The tile set changed, so the whole world regenerates (next render,
        // which holds the `&mut Renderer` the mesh upload needs).
        self.terrain_dirty = true;
    }

    /// Free the old terrain meshes and, when the vocabulary is loaded, run the
    /// six-epoch stack over the current tile set and upload each tile's
    /// top-layer meshes. The across-equator adjacency is read at the rest pose
    /// so wheel state can't change the generated world.
    fn rebuild_terrain(&mut self, renderer: &mut Renderer) {
        for t in self.terrain.drain(..) {
            renderer.free_mesh(t.ground);
            if let Some(w) = t.water {
                renderer.free_mesh(w);
            }
        }
        self.world = None;
        let Some(tables) = self.tables.as_ref() else { return };
        let (m_right, m_left) =
            Self::map_transforms_for(self.num_rings, 0.0, std::f32::consts::PI);
        let world = terrain::WorldGen::generate(
            tables,
            &self.hexes,
            &self.within,
            self.num_rings,
            WORLD_SEED,
            &m_right,
            &m_left,
        );
        self.terrain = world.upload_meshes(tables, &self.hexes, renderer);
        self.world = Some(world);
    }

    /// Screen-X of the slider handle for the current ring count.
    fn slider_handle_x(&self) -> f32 {
        let t = (self.num_rings - MIN_RINGS) as f32 / (MAX_RINGS - MIN_RINGS) as f32;
        SLIDER_X0 + t * (SLIDER_X1 - SLIDER_X0)
    }

    /// Ring count for a cursor at screen-X `x` (snapped to an integer step).
    fn slider_value(&self, x: f32) -> usize {
        let t = ((x - SLIDER_X0) / (SLIDER_X1 - SLIDER_X0)).clamp(0.0, 1.0);
        MIN_RINGS + (t * (MAX_RINGS - MIN_RINGS) as f32).round() as usize
    }

    /// Whether a cursor at `p` is close enough to the slider to grab it.
    fn over_slider(&self, p: Vec2) -> bool {
        p.x >= SLIDER_X0 - SLIDER_HIT_PAD
            && p.x <= SLIDER_X1 + SLIDER_HIT_PAD
            && (p.y - SLIDER_Y).abs() <= SLIDER_HIT_PAD
    }

    /// Build a world-space pick ray (origin, unit dir) through the pixel at
    /// `cursor` for a `viewport`-sized window, from the camera basis. Mirrors
    /// voxel-cluster's `build_pick_ray`.
    fn build_pick_ray(&self, cursor: Vec2, viewport: Vec2) -> (Vec3, Vec3) {
        let f = self.forward();
        let r = f.cross(Vec3::Y).normalize_or_zero();
        let u = r.cross(f).normalize_or_zero();
        let aspect = viewport.x / viewport.y;
        let t = (PICK_FOV_Y * 0.5).tan();
        // +0.5 so the ray passes through the pixel centre.
        let ndc_x = 2.0 * (cursor.x + 0.5) / viewport.x - 1.0;
        let ndc_y = 1.0 - 2.0 * (cursor.y + 0.5) / viewport.y;
        let dir = (f + r * (ndc_x * aspect * t) + u * (ndc_y * t)).normalize_or_zero();
        (self.position, dir)
    }

    /// Cast a ray through `cursor` and return the `number` of the nearest hex
    /// face it hits (resting positions, so hover stays put while the highlight
    /// stretches). `None` if the cursor is over empty space.
    fn pick(&self, cursor: Vec2, viewport: Vec2) -> Option<u32> {
        if viewport.x <= 0.0 || viewport.y <= 0.0 {
            return None;
        }
        let (m_right, m_left) = self.map_transforms();
        let (origin, dir) = self.build_pick_ray(cursor, viewport);
        let mut best: Option<(f32, u32)> = None;
        for inst in &self.hexes {
            let xform = if inst.left { &m_left } else { &m_right };
            let center = xform.transform_point3(inst.center);
            let corners = hex_world_corners(inst.center, inst.flip, inst.dome_center, xform);
            for i in 0..6 {
                if let Some(t) = ray_triangle(origin, dir, center, corners[i], corners[(i + 1) % 6])
                {
                    if best.is_none_or(|(bt, _)| t < bt) {
                        best = Some((t, inst.number));
                    }
                }
            }
        }
        best.map(|(_, n)| n)
    }

    /// World centre of tile `inst` under the current map transforms.
    fn tile_center(&self, inst: &HexInst, m_right: &Mat4, m_left: &Mat4) -> Vec3 {
        if inst.left {
            m_left.transform_point3(inst.center)
        } else {
            m_right.transform_point3(inst.center)
        }
    }

    /// The six neighbours of tile `n`: its same-map neighbours ([`Self::within`])
    /// first, then — if it's an equator tile short of six — the nearest tiles in
    /// the *other* map's equator (the across-equator mesh), measured on the live
    /// rendered centres so the match tracks the maps' rolls.
    fn neighbors_of(&self, n: u32) -> Vec<u32> {
        let within = &self.within[n as usize];
        let mut out = within.clone();
        let need = 6usize.saturating_sub(within.len());
        if need > 0 {
            let (m_right, m_left) = self.map_transforms();
            let here = &self.hexes[n as usize];
            let origin = self.tile_center(here, &m_right, &m_left);
            let mut cands: Vec<(f32, u32)> = self
                .hexes
                .iter()
                .filter(|b| b.left != here.left && self.within[b.number as usize].len() < 6)
                .map(|b| {
                    (
                        (self.tile_center(b, &m_right, &m_left) - origin).length(),
                        b.number,
                    )
                })
                .collect();
            cands.sort_by(|a, b| a.0.total_cmp(&b.0));
            out.extend(cands.into_iter().take(need).map(|(_, num)| num));
        }
        out
    }

    /// The seven-tile highlight set for the current hover: the hovered tile plus
    /// its six neighbours.
    fn compute_highlight(&self) -> Vec<u32> {
        let Some(h) = self.hovered else {
            return Vec::new();
        };
        let mut hi = Vec::with_capacity(7);
        hi.push(h);
        hi.extend(self.neighbors_of(h));
        hi
    }

    /// Lay the selected tile's six neighbours flat into its tangent plane,
    /// aligned to its edges — the "fold-in" for select mode. Each neighbour gets
    /// a hex-step offset in the selected tile `s`'s plane: same-map neighbours
    /// land on the exact edge they share (from the logical offset, flip-corrected
    /// for the left map); across-equator neighbours fill the remaining outward
    /// edges by world-direction match.
    fn build_rosette(&self, s: u32, m_right: &Mat4, m_left: &Mat4) -> Rosette {
        let here = &self.hexes[s as usize];
        let xform = if here.left { *m_left } else { *m_right };
        let tilt = tilt_to_normal((here.center - here.dome_center).normalize_or_zero());
        // Edge direction in the flat frame, mirrored when the map is flipped, so
        // the slot lines up with the tile's *drawn* edge.
        let flip_dir = |v: Vec3| {
            if here.flip {
                Vec3::new(-v.x, v.y, v.z)
            } else {
                v
            }
        };
        let slot_pre = |e: usize| here.center + tilt.transform_vector3(flip_dir(edge_normal(e)) * HEX_SPACING);
        let slots_pre: [Vec3; 6] = std::array::from_fn(slot_pre);
        let w_s = xform.transform_point3(here.center);

        let within = &self.within[s as usize];
        let all = self.neighbors_of(s);
        let mut used = [false; 6];
        let mut out: Vec<(u32, Vec3)> = Vec::with_capacity(6);

        // Same-map neighbours → the edge their logical offset points along.
        for &n in within {
            let o = (self.hexes[n as usize].logical - here.logical).normalize_or_zero();
            let e = (0..6)
                .filter(|&e| !used[e])
                .max_by(|&a, &b| o.dot(edge_normal(a)).total_cmp(&o.dot(edge_normal(b))))
                .unwrap_or(0);
            used[e] = true;
            out.push((n, slots_pre[e]));
        }
        // Across-equator neighbours → the nearest remaining (outward) edge, by
        // world direction from the selected tile.
        for &n in all.iter().filter(|n| !within.contains(n)) {
            let dir_n = (self.tile_center(&self.hexes[n as usize], m_right, m_left) - w_s)
                .normalize_or_zero();
            let e = (0..6)
                .filter(|&e| !used[e])
                .max_by(|&a, &b| {
                    let da = (xform.transform_point3(slots_pre[a]) - w_s).normalize_or_zero();
                    let db = (xform.transform_point3(slots_pre[b]) - w_s).normalize_or_zero();
                    dir_n.dot(da).total_cmp(&dir_n.dot(db))
                })
                .unwrap_or(0);
            used[e] = true;
            out.push((n, slots_pre[e]));
        }
        Rosette {
            center: s,
            left: here.left,
            flip: here.flip,
            tilt,
            slots: out,
        }
    }

    /// Per-frame values handed to the HUD script as the `Model` global.
    fn hud_model(&self) -> ValueMap {
        ValueMap::new()
            .with("pos_x", self.position.x)
            .with("pos_y", self.position.y)
            .with("pos_z", self.position.z)
            .with("yaw", self.yaw)
            .with("pitch", self.pitch)
    }

    /// Draw `text` as camera-facing glyph billboards centred at `center`, laid
    /// out so it reads left→right from this client's overhead framing (see
    /// [`glyph_x`]). Characters outside `CHARSET` are skipped.
    fn draw_text_billboard(
        &self,
        renderer: &mut Renderer,
        atlas: TextureHandle,
        center: Vec3,
        text: &str,
        glyph: f32,
        color: [f32; 4],
    ) {
        let n = text.chars().count();
        for (i, ch) in text.chars().enumerate() {
            let Some(cell) = glyph_cell(ch) else { continue };
            let (uv0, uv1) = glyph_uv(cell);
            let pos = Vec3::new(glyph_x(center.x, i, n, glyph), center.y, center.z);
            renderer.draw_billboard(atlas, pos, Vec2::splat(glyph), uv0, uv1, color);
        }
    }

    /// Draw hexagon `number` centred at `center`: each edge in its `EDGE_COLORS`
    /// hue with its a–f letter at the midpoint, a dot at every corner, and the
    /// number billboard floating over the centre. Every hex shares the same
    /// orientation and edge labelling, so adjacency reads straight off the
    /// colours/letters (e.g. hex 0's edge `a` faces hex 1's edge `d`).
    ///
    /// Draw the ring-count slider (2D, screen-space): a track with a tick per
    /// ring count, a handle at the current value, and a `Rings / Tiles` readout.
    fn draw_slider(&self, renderer: &mut Renderer) {
        let Some(white) = self.white else { return };
        // Track.
        renderer.draw_sprite(
            white,
            Vec2::new(SLIDER_X0, SLIDER_Y - SLIDER_TRACK_H * 0.5),
            Vec2::new(SLIDER_X1 - SLIDER_X0, SLIDER_TRACK_H),
            [0.24, 0.27, 0.33, 0.85],
        );
        // A tick at each integer ring count.
        for n in MIN_RINGS..=MAX_RINGS {
            let t = (n - MIN_RINGS) as f32 / (MAX_RINGS - MIN_RINGS) as f32;
            let x = SLIDER_X0 + t * (SLIDER_X1 - SLIDER_X0);
            renderer.draw_sprite(
                white,
                Vec2::new(x - 1.0, SLIDER_Y - 9.0),
                Vec2::new(2.0, 18.0),
                [0.45, 0.50, 0.58, 0.9],
            );
        }
        // Handle (cyan, matching the guide circles).
        let hx = self.slider_handle_x();
        renderer.draw_sprite(
            white,
            Vec2::new(hx - SLIDER_HANDLE_W * 0.5, SLIDER_Y - SLIDER_HANDLE_H * 0.5),
            Vec2::new(SLIDER_HANDLE_W, SLIDER_HANDLE_H),
            GUIDE_COLOR,
        );
        // Readout above the track.
        let tiles = 2 * (1 + 3 * self.num_rings * (self.num_rings + 1));
        let label = format!("Rings {}   ({} tiles)", self.num_rings, tiles);
        renderer.draw_text(&label, Vec2::new(SLIDER_X0, SLIDER_Y - 32.0), 18.0, [0.90, 0.95, 1.0, 1.0]);
    }

    /// Draw one tile: its translucent fill (brighter `HOVER_TINT` when `lit`)
    /// then its coloured wireframe + labels, both at `center`/`tilt`/`xform`.
    #[allow(clippy::too_many_arguments)]
    fn draw_tile(
        &self,
        renderer: &mut Renderer,
        center: Vec3,
        tilt: Mat4,
        xform: &Mat4,
        flip: bool,
        number: u32,
        lit: bool,
    ) {
        if let Some(t) = self.terrain.get(number as usize) {
            // The world-gen top layer — the Epoch-6 ground relief with its sea
            // — rides the tile's full placement chain, so it stretches on
            // hover and folds into a rosette with the wireframe.
            let model = *xform * Mat4::from_translation(center) * tilt;
            let tint = if lit { TERRAIN_HOVER_TINT } else { [1.0, 1.0, 1.0, 1.0] };
            renderer.draw_mesh(t.ground, model, MeshDrawOptions { wireframe: false, tint });
            if let Some(w) = t.water {
                renderer.draw_mesh(w, model, MeshDrawOptions { wireframe: false, tint });
            }
        } else if let Some(mesh) = self.fill_mesh {
            // Fallback when the vocabulary failed to load: the plain grey fill.
            let model = *xform
                * Mat4::from_translation(center)
                * tilt
                * Mat4::from_translation(Vec3::new(0.0, -FILL_INSET, 0.0));
            let tint = if lit { HOVER_TINT } else { FILL_TINT };
            renderer.draw_mesh(mesh, model, MeshDrawOptions { wireframe: false, tint });
        }
        self.draw_hex(renderer, center, number, flip, xform, tilt);
    }

    /// `tilt` rotates the flat hex about its centre so its face normal points
    /// along its radial (normally [`tilt_to_normal`] of the dome radius, so the
    /// tile lies flat at the apex and vertical at the equator). For a selected
    /// tile's folded-in neighbour, the caller instead passes the *selected*
    /// tile's tilt so the neighbour lies coplanar with it.
    fn draw_hex(
        &self,
        renderer: &mut Renderer,
        center: Vec3,
        number: u32,
        flip: bool,
        xform: &Mat4,
        tilt: Mat4,
    ) {
        let mut corners = hex_corners(center, HEX_SIZE);
        if flip {
            // Record-flip about the N-S axis through the centre: mirror
            // west↔east. b/e stay at N/S; the a/f and c/d edges swap sides and
            // the colour arrangement reverses left-to-right.
            for c in corners.iter_mut() {
                *c = flip_ns(*c, center);
            }
        }
        // Apply `tilt` (about the centre) then the map's roll `xform`. `place`
        // does both, so every drawn point is tilted-then-rolled together.
        let place = |p: Vec3| xform.transform_point3(center + tilt.transform_vector3(p - center));
        for i in 0..6 {
            let a = place(corners[i]);
            let b = place(corners[(i + 1) % 6]);
            let color = EDGE_COLORS[i];
            renderer.draw_lines(&[(a, b)], color);

            if let Some(glyphs) = self.glyphs {
                // Nudge the letter outward from the centre (in the flat frame,
                // correct mirrored too) and lift it off the face, then
                // tilt+roll it with the hex.
                let mid = (corners[i] + corners[(i + 1) % 6]) * 0.5;
                let outward =
                    Vec3::new(mid.x - center.x, 0.0, mid.z - center.z).normalize_or_zero();
                let pos = place(
                    mid + outward * EDGE_LABEL_OUT + Vec3::new(0.0, EDGE_LABEL_SIZE * 0.5, 0.0),
                );
                let letter = (b'a' + i as u8) as char;
                self.draw_text_billboard(
                    renderer,
                    glyphs,
                    pos,
                    &letter.to_string(),
                    EDGE_LABEL_SIZE,
                    color,
                );
            }
        }
        if let Some(dot) = self.dot {
            for c in corners {
                renderer.draw_billboard(
                    dot,
                    place(c),
                    Vec2::splat(DOT_SIZE),
                    Vec2::ZERO,
                    Vec2::ONE,
                    [1.0, 0.85, 0.25, 1.0],
                );
            }
        }
        if let Some(glyphs) = self.glyphs {
            let label = place(center + Vec3::new(0.0, LABEL_SIZE * 0.5 + 40.0, 0.0));
            self.draw_text_billboard(
                renderer,
                glyphs,
                label,
                &number.to_string(),
                LABEL_SIZE,
                [1.0, 0.95, 0.6, 1.0],
            );
        }
    }
}

impl Scene for HexScene {
    fn enter(&mut self, renderer: &mut Renderer) {
        // 1×1 white pixel — tinted to build solid HUD quads in `render_hud`.
        self.white = Some(renderer.load_texture(&[0xff, 0xff, 0xff, 0xff], 1, 1));
        // Soft disc for the corner dots, and the digit atlas for the label.
        self.dot = Some(renderer.load_texture(&build_disc_texture(), 16, 16));
        self.glyphs = Some(renderer.load_texture(
            &build_glyph_atlas(),
            ATLAS_W as u32,
            ATLAS_H as u32,
        ));
        // Unit translucent hex face, instanced per tile via draw_mesh.
        let (verts, idx) = build_hex_fill_mesh();
        self.fill_mesh = Some(renderer.upload_mesh(&verts, MeshIndices::U32(&idx)));

        // Scripted HUD: layout in ui_elements.json (`UI.hud`), behaviour in Lua.
        match ScriptHost::from_file(HUD_SCRIPT_PATH) {
            Ok(s) => {
                load_ui_json(&s, UI_ELEMENTS_PATH);
                load_widgets(&s);
                tracing::info!("loaded HUD script from {HUD_SCRIPT_PATH}");
                self.script = Some(s);
            }
            Err(e) => tracing::error!("HUD script load failed: {e}"),
        }
    }

    fn update(&mut self, dt: Duration, input: &InputState, renderer: &Renderer) -> Transition {
        // The wasd_and_mouse preset binds Escape to Menu (its later Escape bind
        // wins under the one-input-one-action rule), so check the key directly
        // for this menu-less demo.
        if input.key_down(Key::Escape) {
            return Transition::Quit;
        }
        let dt_s = dt.as_secs_f32();

        // Roll wheels: left-click a wheel to grab it, drag vertically to roll its
        // map about the N-S axis (clamped to 0..π). Hit-tested by projecting each
        // wheel centre to the screen.
        let screen = renderer.size();
        let aspect = if screen.y > 0.0 { screen.x / screen.y } else { 1.0 };
        let view_proj = Camera {
            position: self.position,
            target: self.position + self.forward(),
            up: Vec3::Y,
            fov_y_radians: 60.0_f32.to_radians(),
            near: 1.0,
            far: 100000.0,
        }
        .view_projection(aspect);
        let right_wheel = Vec3::new(0.0, 0.0, RIGHT_WHEEL_Z);
        let left_wheel = Vec3::new(left_axis_x(), 0.0, LEFT_WHEEL_Z);
        if input.mouse_left_pressed {
            if self.over_slider(input.mouse_position) {
                // Grab the ring-count slider — this press neither rolls nor picks.
                self.slider_drag = true;
                self.set_rings(self.slider_value(input.mouse_position.x));
            } else {
                let hit = |w: Vec3| {
                    project_to_screen(w, view_proj, screen)
                        .is_some_and(|sp| (sp - input.mouse_position).length() < WHEEL_PICK_PX)
                };
                self.drag = if hit(right_wheel) {
                    Some(Wheel::Right)
                } else if hit(left_wheel) {
                    Some(Wheel::Left)
                } else {
                    None
                };
                self.drag_cursor = input.mouse_position;
                // A click that didn't grab a wheel toggles tile selection: pick
                // the tile under the cursor; clicking the selected tile again
                // clears it, clicking another selects it, empty space clears it.
                if self.drag.is_none() {
                    self.selected = match self.pick(input.mouse_position, screen) {
                        Some(n) if self.selected == Some(n) => None,
                        other => other,
                    };
                }
            }
        }
        if input.mouse_left {
            if self.slider_drag {
                self.set_rings(self.slider_value(input.mouse_position.x));
            } else if let Some(wheel) = self.drag {
                let delta = (input.mouse_position.y - self.drag_cursor.y) * ROLL_SENS;
                self.drag_cursor = input.mouse_position;
                let roll = match wheel {
                    Wheel::Right => &mut self.right_roll,
                    Wheel::Left => &mut self.left_roll,
                };
                *roll = (*roll + delta).clamp(0.0, std::f32::consts::PI);
            }
        } else {
            self.drag = None;
            self.slider_drag = false;
        }

        // Look: right-drag, with sensitivity/invert applied by the config.
        if input.mouse_right {
            if let Some(prev) = self.last_look_cursor {
                let (dyaw, dpitch) = self.controls.look_delta_mouse(input.mouse_position - prev);
                self.yaw -= dyaw;
                self.pitch = (self.pitch + dpitch).clamp(-1.5, 1.5);
            }
            self.last_look_cursor = Some(input.mouse_position);
        } else {
            self.last_look_cursor = None;
        }

        // Move: free 6-DOF fly in the camera's facing.
        let mut motion = Vec3::ZERO;
        if input.input_active(&self.bindings, Action::MoveForward) {
            motion += self.move_forward();
        }
        if input.input_active(&self.bindings, Action::MoveBackward) {
            motion -= self.move_forward();
        }
        if input.input_active(&self.bindings, Action::StrafeRight) {
            motion += self.move_right();
        }
        if input.input_active(&self.bindings, Action::StrafeLeft) {
            motion -= self.move_right();
        }
        if input.input_active(&self.bindings, Action::MoveUp) {
            motion += Vec3::Y;
        }
        if input.input_active(&self.bindings, Action::MoveDown) {
            motion -= Vec3::Y;
        }
        if motion.length_squared() > 0.0 {
            self.position += motion.normalize() * self.controls.move_speed * dt_s;
        }

        // Hover hit-test, rate-limited to PICK_HZ (not every frame): ray-cast the
        // cursor against the tile faces and remember which tile is under it. The
        // hovered tile is stretched outward in `render`.
        self.pick_accum += dt_s;
        if self.pick_accum >= 1.0 / PICK_HZ {
            self.pick_accum = 0.0;
            // Don't hover-pick through the slider widget.
            self.hovered = if self.over_slider(input.mouse_position) {
                None
            } else {
                self.pick(input.mouse_position, screen)
            };
            self.highlight = self.compute_highlight();
        }

        Transition::None
    }

    fn render(&mut self, renderer: &mut Renderer) {
        // (Re)generate the world + terrain meshes when the tile set changed —
        // here because the mesh upload needs the `&mut Renderer`.
        if self.terrain_dirty {
            self.terrain_dirty = false;
            self.rebuild_terrain(renderer);
        }

        renderer.set_camera(&Camera {
            position: self.position,
            target: self.position + self.forward(),
            up: Vec3::Y,
            fov_y_radians: 60.0_f32.to_radians(),
            near: 1.0,
            far: 100000.0,
        });

        // A plain default sky gives a horizon to orient against.
        renderer.set_scene(SceneLighting::default());
        renderer.draw_sky();

        // Both maps' roll/placement transforms (see `map_transforms`): the right
        // map rolls about world Z; the left map rolls the opposite way, drops
        // below, yaws a half-hex, and slides under the right map. Shared with the
        // mouse pick so drawing and hit-test agree.
        let lx = left_axis_x();
        let (m_right, m_left) = self.map_transforms();
        // Mirror west↔east (keeping Y) — for the left compass and guides below.
        let reflect = |p: Vec3| Vec3::new(2.0 * LEFT_AXIS_X - p.x, p.y, p.z);
        let c = left_center();

        // Compass at each map's centre.
        draw_compass(renderer, Vec3::ZERO, false, &m_right);
        draw_compass(renderer, reflect(c), true, &m_left);

        // When a tile is clicked/selected, fold its six neighbours flat into its
        // tangent plane, aligned to its edges.
        let rosette = self.selected.map(|s| self.build_rosette(s, &m_right, &m_left));

        // Every tile of both maps: a 25%-grey translucent face (the pickable
        // surface) plus its coloured wireframe + labels (`draw_tile`). A
        // selected tile's neighbours fold in (rosette); otherwise the hovered
        // tile and its six neighbours stretch outward along their radial — the
        // hover highlight. The fill insets a hair so edges draw in front.
        for inst in &self.hexes {
            let own = if inst.left { &m_left } else { &m_right };
            if let Some(ros) = &rosette {
                // A folded-in neighbour: draw in the selected tile's plane.
                if let Some(&(_, slot)) = ros.slots.iter().find(|(num, _)| *num == inst.number) {
                    let rx = if ros.left { &m_left } else { &m_right };
                    self.draw_tile(renderer, slot, ros.tilt, rx, ros.flip, inst.number, true);
                    continue;
                }
                // The selected tile itself: its resting spot, highlighted (anchor
                // of the rosette).
                if ros.center == inst.number {
                    let tilt = tilt_to_normal((inst.center - inst.dome_center).normalize_or_zero());
                    self.draw_tile(renderer, inst.center, tilt, own, inst.flip, inst.number, true);
                    continue;
                }
            }
            // Hover highlight: the lit tile pushes outward along its radial.
            let lit = self.highlight.contains(&inst.number);
            let center = if lit {
                let radial = (inst.center - inst.dome_center).normalize_or_zero();
                inst.center + radial * HOVER_STRETCH
            } else {
                inst.center
            };
            let tilt = tilt_to_normal((center - inst.dome_center).normalize_or_zero());
            self.draw_tile(renderer, center, tilt, own, inst.flip, inst.number, lit);
        }

        // Ring guide circles + cardinal dome, per map. Each ring circle passes
        // through that ring's CORNER hexes (radius k·HEX_SPACING); the
        // straight-run hexes between corners on ring 2+ fall just inside and are
        // skipped. Over each map a cardinal dome of the outer radius (3·s) arcs
        // across the X (E–W) and Z (N–S) verticals, springing from the outer
        // ring's four cardinal points up to an apex over the centre — so the
        // dome diameter equals the outer ring's (e.g. the right map's 21↔30
        // span). Centred on each map's roll axis (right at the origin, left at
        // its reflected centre column), lifted +Y, rolled with the map.
        use std::f32::consts::{PI, TAU};
        let rings = self.num_rings;
        let outer_r = dome_radius(rings);
        for (centre, xform) in [(Vec3::ZERO, &m_right), (reflect(c), &m_left)] {
            // Latitude rings: each ring circle sits at its even angular division
            // down the dome (radius ring_dome_radius, height ring_dome_lift),
            // threading the ring of hexes resting there.
            for k in 1..=rings {
                let lat = centre + Vec3::new(0.0, ring_dome_lift(k, rings), 0.0);
                draw_arc(renderer, lat, ring_dome_radius(k, rings), Vec3::X, Vec3::Z, 0.0, TAU, GUIDE_SEGS, xform, GUIDE_COLOR);
            }
            // Cardinal meridians: half-arcs over the X (E–W) and Z (N–S)
            // verticals, from the base equator up over the apex.
            let base = centre + Vec3::new(0.0, GUIDE_LIFT, 0.0);
            draw_arc(renderer, base, outer_r, Vec3::X, Vec3::Y, 0.0, PI, GUIDE_SEGS, xform, GUIDE_COLOR);
            draw_arc(renderer, base, outer_r, Vec3::Z, Vec3::Y, 0.0, PI, GUIDE_SEGS, xform, GUIDE_COLOR);
        }

        // Roll wheels, south of each map on its N-S axis (left-drag to roll).
        draw_wheel(renderer, Vec3::new(0.0, 0.0, RIGHT_WHEEL_Z), WHEEL_RADIUS, self.right_roll, [0.95, 0.85, 0.45, 1.0]);
        draw_wheel(renderer, Vec3::new(lx, 0.0, LEFT_WHEEL_Z), WHEEL_RADIUS, -self.left_roll, [0.95, 0.85, 0.45, 1.0]);

        // Scripted HUD: publish the live model, then draw the script's commands.
        if let (Some(script), Some(white)) = (self.script.as_ref(), self.white) {
            let screen = renderer.size();
            if let Err(e) = script.set_model(&self.hud_model()) {
                tracing::error!("HUD model publish failed: {e}");
            }
            match script.draw(screen.x, screen.y) {
                Ok(cmds) => render_hud(renderer, &cmds, white, &[]),
                Err(e) => tracing::error!("HUD draw failed: {e}"),
            }
        }

        // Ring-count slider, on top of the HUD.
        renderer.set_layer(1000.0);
        self.draw_slider(renderer);
    }
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
                "hex_map=info,flicker_app=info,flicker_render=warn".into()
            }),
        )
        .init();

    run(SceneManager::new(Box::new(HexScene::new())))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    //! Load the *real* HUD script + layout and run a frame, so a Lua
    //! syntax/runtime error (or a `UI.hud` key the script reads but the JSON
    //! forgot to define) fails the build rather than only the running app.
    use super::*;
    use flicker::app::InputState;

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

        // The chart is drawn reflected to the west (screen-left).
        assert!(LEFT_AXIS_X > 0.0);
    }

    #[test]
    fn dome_placement() {
        use std::f32::consts::FRAC_PI_2;
        let rings = 3;
        let r = dome_radius(rings); // 3·s

        // The rings divide the dome's quarter-turn evenly: 30°, 60°, 90°.
        assert!((ring_dome_angle(1, rings) - FRAC_PI_2 / 3.0).abs() < 1e-6);
        assert!((ring_dome_angle(2, rings) - 2.0 * FRAC_PI_2 / 3.0).abs() < 1e-6);
        assert!((ring_dome_angle(3, rings) - FRAC_PI_2).abs() < 1e-6);

        // Outer ring on the base equator (radius r, lift GUIDE_LIFT); centre on
        // the apex (radius 0, lift GUIDE_LIFT + r).
        assert!((ring_dome_radius(3, rings) - r).abs() < 1e-2);
        assert!((ring_dome_lift(3, rings) - GUIDE_LIFT).abs() < 1e-2);
        assert!(ring_dome_radius(0, rings).abs() < 1e-3);
        assert!((ring_dome_lift(0, rings) - (GUIDE_LIFT + r)).abs() < 1e-2);

        // Inner rings step in/down by their angle: ring 1 at 30° (radius r·sin30
        // = r/2), ring 2 at 60°. Heights descend apex → base.
        assert!((ring_dome_radius(1, rings) - r * 0.5).abs() < 1e-2);
        assert!((ring_dome_radius(2, rings) - r * (2.0 * FRAC_PI_2 / 3.0).sin()).abs() < 1e-2);
        assert!(ring_dome_lift(1, rings) > ring_dome_lift(2, rings));
        assert!(ring_dome_lift(2, rings) > ring_dome_lift(3, rings));

        // A placed tile keeps its offset direction, lands on its ring's dome
        // circle, and gains its ring's lift. (HEX_STEPS[10] = 2b, ring-2 corner.)
        let off = hex_center(HEX_STEPS[10]);
        let p = dome_position(off, rings);
        let horiz = Vec3::new(p.x, 0.0, p.z);
        assert!((horiz.length() - ring_dome_radius(2, rings)).abs() < 1e-2);
        assert!((p.y - ring_dome_lift(2, rings)).abs() < 1e-2);
        assert!((horiz.normalize() - off.normalize()).length() < 1e-3); // purely radial

        // A non-corner ring-2 edge tile snaps onto the SAME ring circle.
        let edge = dome_position(hex_center(HEX_STEPS[7]), rings); // due-west edge tile
        assert!((Vec3::new(edge.x, 0.0, edge.z).length() - ring_dome_radius(2, rings)).abs() < 1e-2);

        // The centre tile lands on the apex over the centre axis.
        let apex = dome_position(Vec3::ZERO, rings);
        assert!(apex.x.abs() < 1e-3 && apex.z.abs() < 1e-3);
        assert!((apex.y - (GUIDE_LIFT + r)).abs() < 1e-2);
    }

    #[test]
    fn hex_tilt() {
        // The tilt maps the flat hex normal (+Y) onto the requested normal, and
        // is a rigid rotation (preserves lengths).
        let check = |n: Vec3| {
            let m = tilt_to_normal(n);
            assert!((m.transform_vector3(Vec3::Y) - n).length() < 1e-4, "normal {n:?}");
            let v = Vec3::new(HEX_SIZE, 0.0, 0.0);
            assert!((m.transform_vector3(v).length() - HEX_SIZE).abs() < 1e-1);
        };
        check(Vec3::Y); // apex: identity (stays flat)
        check(Vec3::X); // equator (due west): stands vertical
        check(Vec3::new(0.3, 0.8, -0.5).normalize()); // an oblique radius

        // A ring-3 hex sits at the equator, so its dome radius is horizontal and
        // the hex stands vertical; the centre tile's radius is +Y, so it stays
        // flat. (Both measured against the dome centre at the base height.)
        let dome_c = Vec3::new(0.0, GUIDE_LIFT, 0.0);
        let n3 = (dome_position(first_ring(3)[0], 3) - dome_c).normalize();
        assert!(n3.y.abs() < 1e-3, "ring-3 hex should stand vertical");
        let nc = (dome_position(Vec3::ZERO, 3) - dome_c).normalize();
        assert!((nc - Vec3::Y).length() < 1e-4, "centre hex should lie flat");
    }

    #[test]
    fn seven_tile_highlight() {
        let mut s = HexScene::new();

        // Interior tiles (right centre + ring 1 + ring 2 = 0..=18) have all six
        // neighbours within their own map.
        for n in 0..=18u32 {
            assert_eq!(s.within[n as usize].len(), 6, "interior tile {n}");
        }
        // Equator tiles (right ring 3 = 19..=36) come up short — hex-ring corners
        // keep 3 same-map neighbours, straight-run edges keep 4.
        for n in 19..=36u32 {
            let w = s.within[n as usize].len();
            assert!(w == 3 || w == 4, "equator tile {n} within = {w}");
        }

        // Hovering any tile lights exactly seven (itself + six neighbours): the
        // shortfall on equator tiles is made up across the equator.
        for n in 0..s.hexes.len() as u32 {
            s.hovered = Some(n);
            let hl = s.compute_highlight();
            assert_eq!(hl.len(), 7, "tile {n} highlight {hl:?}");
            assert!(hl.contains(&n), "tile {n} not in its own highlight");
            assert!(hl[1..].iter().all(|&m| m != n), "tile {n} duplicated");
        }
    }

    #[test]
    fn select_rosette() {
        let s = HexScene::new();
        let (mr, ml) = s.map_transforms();
        for n in 0..s.hexes.len() as u32 {
            let ros = s.build_rosette(n, &mr, &ml);
            assert_eq!(ros.center, n);
            // The fold-in places exactly the tile's six neighbours...
            assert_eq!(ros.slots.len(), 6, "tile {n} rosette size");
            let mut got: Vec<u32> = ros.slots.iter().map(|(m, _)| *m).collect();
            got.sort();
            let mut want = s.neighbors_of(n);
            want.sort();
            assert_eq!(got, want, "tile {n} rosette ≠ its neighbours");
            // ...each on a distinct edge slot (no two neighbours stacked).
            for i in 0..6 {
                for j in (i + 1)..6 {
                    assert!(
                        (ros.slots[i].1 - ros.slots[j].1).length() > 1.0,
                        "tile {n} slots {i},{j} coincide"
                    );
                }
            }
        }
    }

    #[test]
    fn dynamic_ring_counts() {
        let mut s = HexScene::new();
        for rings in MIN_RINGS..=MAX_RINGS {
            s.set_rings(rings);
            assert_eq!(s.num_rings, rings);
            // Two maps, each a centre + `rings` rings of 6k tiles.
            let expect = 2 * (1 + 3 * rings * (rings + 1));
            assert_eq!(s.hexes.len(), expect, "{rings} rings → tile count");
            // The seven-tile invariant survives at every ring count.
            for n in 0..s.hexes.len() as u32 {
                s.hovered = Some(n);
                assert_eq!(s.compute_highlight().len(), 7, "{rings} rings, tile {n}");
            }
        }
        // One ring per map is the "map of 14": two centres + two rings of six.
        s.set_rings(1);
        assert_eq!(s.hexes.len(), 14);
        // Clamped to the slider range.
        s.set_rings(99);
        assert_eq!(s.num_rings, MAX_RINGS);
        s.set_rings(0);
        assert_eq!(s.num_rings, MIN_RINGS);
    }

    #[test]
    fn roll() {
        use std::f32::consts::{FRAC_PI_2, PI};

        // Roll 0 is the identity.
        let p = Vec3::new(300.0, 90.0, 50.0);
        assert!((roll_transform(0.0, 0.0).transform_point3(p) - p).length() < 1e-3);

        // Roll π about X=0 is a 180° flip: (x, y, z) → (−x, −y, z).
        let q = roll_transform(0.0, PI).transform_point3(p);
        assert!((q - Vec3::new(-300.0, -90.0, 50.0)).length() < 1e-1);

        // The maps roll OPPOSITE ways (the left map's angle is negated). At the
        // half-roll the right map's surface "up" (+Y) tilts to −X (east) and the
        // left map's to +X (west) — i.e. each tilts away from the other; their
        // bottoms meet. If they shared a sign they'd tilt the same way.
        let right_up = roll_transform(0.0, FRAC_PI_2).transform_point3(Vec3::Y);
        let left_up = roll_transform(0.0, -FRAC_PI_2).transform_point3(Vec3::Y);
        assert!(right_up.x < -0.9 && right_up.y.abs() < 0.1); // right top → east
        assert!(left_up.x > 0.9 && left_up.y.abs() < 0.1); // left top → west (opposite)
        assert!((right_up.x - left_up.x).abs() > 1.5); // genuinely opposite, not the same

        // Left map: a full roll turns the record-flipped tile 19 back upright —
        // its X lands at the un-reflected position, flat again. (±π land the same
        // place; the negation only flips the *path*, not the endpoint.)
        let lx = left_axis_x();
        let logical19 = hex_center(LEFT_MAP_STEPS[0]);
        let reflected19 = Vec3::new(2.0 * LEFT_AXIS_X - logical19.x, 0.0, logical19.z);
        let rolled = roll_transform(lx, -PI).transform_point3(reflected19);
        let upright_x = lx + (logical19.x - hex_center(LEFT_MAP_STEPS[18]).x);
        assert!((rolled.x - upright_x).abs() < 1e-1 && rolled.y.abs() < 1e-2);
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

    #[test]
    fn label_reads_left_to_right() {
        // Under +X = west (screen-left), the first character must sit at a
        // greater X than later ones, so "10" renders "10" not "01".
        let (g, cx) = (100.0, 0.0);
        assert!(glyph_x(cx, 0, 2, g) > glyph_x(cx, 1, 2, g));
        // Two glyphs straddle the centre symmetrically; one glyph stays centred.
        assert!((glyph_x(cx, 0, 2, g) + glyph_x(cx, 1, 2, g)) / 2.0 - cx < 1e-6);
        assert!((glyph_x(7.0, 0, 1, g) - 7.0).abs() < 1e-6);
    }

    #[test]
    fn hud_script_runs() {
        let host = ScriptHost::from_file(HUD_SCRIPT_PATH).expect("load hud.lua");
        load_ui_json(&host, UI_ELEMENTS_PATH); // exposes `UI.hud`
        load_widgets(&host);

        let model = ValueMap::new()
            .with("pos_x", 1.0_f32)
            .with("pos_y", 2.0_f32)
            .with("pos_z", 3.0_f32)
            .with("yaw", 0.5_f32)
            .with("pitch", -0.25_f32);
        host.set_model(&model).expect("publish model");

        let input = InputState::new();
        host.update(&input, 1280.0, 720.0).expect("update runs");
        let cmds = host.draw(1280.0, 720.0).expect("draw runs");
        assert!(!cmds.is_empty(), "HUD emits draw commands");
    }
}
