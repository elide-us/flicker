//! The client's bitmap text: a tiny 5×7 font baked into a glyph atlas, the soft
//! corner-dot texture, and the camera-facing billboard that lays a label out so
//! it reads left→right under the `+X = west` overhead framing. Self-contained —
//! the only public surface is the two texture builders and `draw_text_billboard`.

use flicker::render::{Renderer, TextureHandle, Vec2, Vec3};

fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// A soft white disc on transparent black — billboarded at each hex corner.
pub fn build_disc_texture() -> Vec<u8> {
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

/// Build the RGBA8 glyph atlas — one scaled-up white glyph per `CHARSET` cell on
/// transparent black — with its `(width, height)` in pixels, so the atlas size
/// stays owned here.
pub fn build_glyph_atlas() -> (Vec<u8>, u32, u32) {
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
    (pixels, ATLAS_W as u32, ATLAS_H as u32)
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

/// Draw `text` as camera-facing glyph billboards centred at `center`, laid out
/// so it reads left→right from this client's overhead framing (see [`glyph_x`]).
/// Characters outside `CHARSET` are skipped.
pub fn draw_text_billboard(
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
