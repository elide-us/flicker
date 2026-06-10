//! hex-map: the first brick of the world hex-map client. It establishes the
//! coordinate frame the map ordering will be built on top of, by drawing:
//!
//!   * an **XYZ compass** through the origin — red X, green Y, blue Z, with the
//!     world convention `+X = west, +Y = up, +Z = north` (right-handed, Y-up,
//!     matching voxel-cluster); each axis's positive half is bright with a
//!     pyramid arrowhead, the negative half dim, so `+`/`-` reads at a glance;
//!   * **flat-top hexagons**, 2048 units corner-to-corner (the two points face
//!     east/west: +X = west, −X = east), drawn as rasterized line wireframes on
//!     the ground plane (XZ, y = 0) with a **dot at each corner**, each edge in
//!     its own colour and labelled `a`–`f` (clockwise from the west point);
//!   * a **number billboard** over each hex centre — the per-hex map-ordering
//!     label. Hex 0 sits at the origin; ring 1 (1–6) is one neighbour across
//!     each edge `a`–`f` (clockwise: 1 NW, 2 N, 3 NE, 4 SE, 5 S, 6 SW); ring 2
//!     (7–18) is the next ring, walked clockwise from due west (hex 7). Every
//!     neighbour is drawn with a gap.
//!   * a **second "left map"** — tiles 19–37 split off into a separate chart to
//!     the west (screen-left), **record-flipped** about the N-S axis (Y down,
//!     west↔east mirrored; b/e stay N/S): a full 19-tile hexagon — ring-2
//!     (19–30), ring-1 (31–36), centre tile (37). A second compass, sitting on
//!     the centre tile, marks the flipped frame.
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
use flicker::render::{Camera, Renderer, SceneLighting, TextureHandle, Vec2, Vec3};
use flicker::scene::{Scene, SceneManager, Transition};
use flicker::script::{ScriptHost, ValueMap};
use flicker::ui::{load_ui_json, load_widgets, render_hud};

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
const LEFT_AXIS_X: f32 = 3.5 * HEX_SPACING;

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
/// origin — the sum of the step directions scaled to the hex spacing.
fn hex_center(steps: &[usize]) -> Vec3 {
    steps
        .iter()
        .fold(Vec3::ZERO, |acc, &s| acc + edge_normal(s))
        * HEX_SPACING
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
/// the frame is record-flipped about the N-S axis — **X points east, Y points
/// down**, Z/north unchanged — matching the flipped left-chart tiles.
fn draw_compass(renderer: &mut Renderer, origin: Vec3, flip: bool) {
    let axes = [
        (Vec3::X, [0.95, 0.30, 0.30, 1.0]),
        (Vec3::Y, [0.40, 0.90, 0.45, 1.0]),
        (Vec3::Z, [0.40, 0.62, 1.00, 1.0]),
    ];
    for (dir, color) in axes {
        let d = if flip {
            Vec3::new(-dir.x, -dir.y, dir.z)
        } else {
            dir
        };
        let dim = [color[0] * 0.4, color[1] * 0.4, color[2] * 0.4, 0.7];
        renderer.draw_lines(&[(origin, origin - d * AXIS_LEN)], dim);
        renderer.draw_lines(&[(origin, origin + d * AXIS_LEN)], color);
        renderer.draw_lines(&arrowhead(origin + d * AXIS_LEN, d, ARROW), color);
    }
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

    // GPU handles, uploaded once in `enter`.
    white: Option<TextureHandle>,
    dot: Option<TextureHandle>,
    glyphs: Option<TextureHandle>,
    script: Option<ScriptHost>,
}

impl HexScene {
    fn new() -> Self {
        Self {
            // Above and south, angled down — framed to take in the first map
            // (right) and the flipped left arch 19–27 (screen-left).
            position: Vec3::new(6600.0, 8500.0, -11500.0),
            yaw: 0.0,     // face +Z (north)
            pitch: -0.58, // look down at it
            last_look_cursor: None,
            bindings: InputMap::wasd_and_mouse(),
            controls: AbstractControls {
                // Two charts side by side (~20 k across) — fly fast.
                move_speed: 2500.0,
                ..AbstractControls::default()
            },
            white: None,
            dot: None,
            glyphs: None,
            script: None,
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
    fn draw_hex(&self, renderer: &mut Renderer, center: Vec3, number: u32, flip: bool) {
        let mut corners = hex_corners(center, HEX_SIZE);
        if flip {
            // Record-flip about the N-S axis through the centre: mirror
            // west↔east. b/e stay at N/S; the a/f and c/d edges swap sides and
            // the colour arrangement reverses left-to-right.
            for c in corners.iter_mut() {
                *c = flip_ns(*c, center);
            }
        }
        for i in 0..6 {
            let a = corners[i];
            let b = corners[(i + 1) % 6];
            let color = EDGE_COLORS[i];
            renderer.draw_lines(&[(a, b)], color);

            if let Some(glyphs) = self.glyphs {
                // Nudge the letter outward from the centre (correct mirrored too)
                // and lift it off the ground plane.
                let mid = (a + b) * 0.5;
                let outward =
                    Vec3::new(mid.x - center.x, 0.0, mid.z - center.z).normalize_or_zero();
                let pos = mid + outward * EDGE_LABEL_OUT + Vec3::new(0.0, EDGE_LABEL_SIZE * 0.5, 0.0);
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
                    c,
                    Vec2::splat(DOT_SIZE),
                    Vec2::ZERO,
                    Vec2::ONE,
                    [1.0, 0.85, 0.25, 1.0],
                );
            }
        }
        if let Some(glyphs) = self.glyphs {
            let label = center + Vec3::new(0.0, LABEL_SIZE * 0.5 + 40.0, 0.0);
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

    fn update(&mut self, dt: Duration, input: &InputState, _renderer: &Renderer) -> Transition {
        // The wasd_and_mouse preset binds Escape to Menu (its later Escape bind
        // wins under the one-input-one-action rule), so check the key directly
        // for this menu-less demo.
        if input.key_down(Key::Escape) {
            return Transition::Quit;
        }
        let dt_s = dt.as_secs_f32();

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

        Transition::None
    }

    fn render(&mut self, renderer: &mut Renderer) {
        renderer.set_camera(&Camera {
            position: self.position,
            target: self.position + self.forward(),
            up: Vec3::Y,
            fov_y_radians: 60.0_f32.to_radians(),
            near: 1.0,
            far: 50000.0,
        });

        // A plain default sky gives a horizon to orient against.
        renderer.set_scene(SceneLighting::default());
        renderer.draw_sky();

        // Compass gadget at the world origin for the first map.
        draw_compass(renderer, Vec3::ZERO, false);

        // The first map, drawn from `HEX_STEPS` (each hex's path of unit edge-
        // steps from the origin, indexed by its number): hex 0 at the centre,
        // ring 1 (1–6) across edges a–f, ring 2 (7–18) walked clockwise from due
        // west. Every hex shares the orientation/edge-labelling, so a hex's edge
        // meets its neighbour's opposite edge across the gap (0.a ↔ 1.d, …).
        for (number, steps) in HEX_STEPS.iter().enumerate() {
            self.draw_hex(renderer, hex_center(steps), number as u32, false);
        }

        // The left map: ring-3 tiles 19–24, the whole chart **record-flipped**
        // about `LEFT_AXIS_X` — logical X reflects through the axis (landing west
        // of the first map) and each tile mirrors (flip=true), which together is
        // one clean reflection. 19–21 are the seam column; 22–24 trace the ring-3
        // NW→N edge. All links back to the first map are logical.
        for (i, steps) in LEFT_MAP_STEPS.iter().enumerate() {
            let logical = hex_center(steps);
            let pos = Vec3::new(2.0 * LEFT_AXIS_X - logical.x, 0.0, logical.z);
            self.draw_hex(renderer, pos, (19 + i) as u32, true);
        }
        // Second compass gadget for the flipped chart (Y down, X east), moved to
        // sit on the centre tile 37 (its X already matches the 23/29 column; now
        // lifted north to the centre's Z). The map itself isn't repositioned.
        let centre = hex_center(LEFT_MAP_STEPS[18]); // [18] = tile 37 (centre)
        draw_compass(renderer, Vec3::new(2.0 * LEFT_AXIS_X - centre.x, 0.0, centre.z), true);

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
