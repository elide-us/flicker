//! The node kinds' own maths: the closed-form generators, the pointwise filters, and
//! the raster filters that read a neighbourhood. Every generator is periodic on the
//! tile and every neighbourhood read wraps — the two halves of seamlessness, as in
//! the baker. Where a node IS v1 maths (occlusion, the ring, the band, modulation)
//! it calls the baker's function instead of restating it.

use std::f64::consts::{FRAC_1_SQRT_2, TAU};

use flicker_primitive::noise;

use super::eval::Plane;
use super::{Axis, Pattern};
use crate::{bake, channel};

/// How far a shape's face rises from its mortar, in cells: a bevel wide enough for
/// the normal map to catch, narrow enough that a tile still reads as flat-topped.
const BEVEL: f64 = 0.1;

/// How hard [`curvature`] maps a texel's lift over its ring: a crest a sixth of the
/// field's range above its surroundings reaches white (a cavity as deep, black).
const CURVATURE_GAIN: f32 = 3.0;

/// The course pitch of regular hexagons one column apart: `√3 / 2`.
const HEX_ROW: f64 = 0.866_025_403_784_438_6;

/// Position within a lattice cell, `[0, 1)`, wrapping negatives the right way.
fn cell(t: f64) -> f64 {
    t.rem_euclid(1.0)
}

/// A radius given as a fraction of the tile, in whole texels at `n` — so a blur or a
/// curvature means the same thing at every bake size.
fn in_texels(radius: f64, n: usize) -> usize {
    (radius.max(0.0) * n as f64).round() as usize
}

/// `f(x, y)` over the tile, row-major — the raster walk of the neighbourhood nodes.
fn raster_xy(n: u32, mut f: impl FnMut(i64, i64) -> f64) -> Vec<f64> {
    let mut out = Vec::with_capacity((n as usize) * (n as usize));
    for y in 0..n as i64 {
        for x in 0..n as i64 {
            out.push(f(x, y));
        }
    }
    out
}

/// A mirrored ramp `repeat` times across the tile, or a cone from each cell's centre.
pub(crate) fn gradient(axis: Axis, repeat: u32, u: f64, v: f64) -> f64 {
    let r = repeat.max(1) as f64;
    match axis {
        // A sawtooth would tear at the seam; folded about its midpoint
        // (`noise::ridged`) it is the mirrored ramp that closes it.
        Axis::U => noise::ridged(cell(u * r)),
        Axis::V => noise::ridged(cell(v * r)),
        Axis::Radial => {
            let (x, y) = (cell(u * r) - 0.5, cell(v * r) - 0.5);
            // 1 at the centre, 0 at the corners. The distance is symmetric about every
            // cell edge, so neighbouring cells — and the seam — meet level.
            (1.0 - (x * x + y * y).sqrt() / FRAC_1_SQRT_2).clamp(0.0, 1.0)
        }
    }
}

/// A tileable pattern of `cells` across: 0 in the mortar, rising across [`BEVEL`] to
/// 1 on the face. `mortar` is the gap's full width, as a fraction of a cell.
pub(crate) fn shape(pattern: Pattern, cells: u32, mortar: f64, u: f64, v: f64) -> f64 {
    let c = cells.max(1) as f64;
    let half_gap = mortar.clamp(0.0, 0.9) * 0.5;
    // `e` is the distance from the cell's border, in cells.
    let face = |e: f64| ((e - half_gap) / BEVEL).clamp(0.0, 1.0);
    match pattern {
        Pattern::Tiles => {
            let (x, y) = (cell(u * c), cell(v * c));
            face(x.min(1.0 - x).min(y).min(1.0 - y))
        }
        Pattern::Bricks => {
            // Twice as many courses as bricks across, so a brick is twice as long as it
            // is tall, and an EVEN course count, so the half-brick stagger closes
            // across the seam.
            let course = v * c * 2.0;
            let shift = if course.floor().rem_euclid(2.0) >= 1.0 {
                0.5
            } else {
                0.0
            };
            let (x, y) = (cell(u * c + shift), cell(course));
            // Measured in course heights, so the mortar is as wide on the ends as on
            // the beds: a brick is two of them long.
            face((x.min(1.0 - x) * 2.0).min(y.min(1.0 - y)))
        }
        Pattern::Hex => face(0.5 - hex_distance(u * c, v, c)),
        Pattern::Dots => {
            let (x, y) = (cell(u * c) - 0.5, cell(v * c) - 0.5);
            face(0.5 - (x * x + y * y).sqrt())
        }
    }
}

/// How far `(x, v)` sits from the centre of its hexagon, in the hexagon's own metric:
/// 0 at the centre, 0.5 on every edge. `x` is in columns (`c` across the tile).
///
/// Pointy-top hexagons in offset courses, one column apart. Regular hexagons want
/// `c / (√3/2)` courses; the stagger only closes across the seam on an EVEN count, so
/// the honeycomb takes the nearest even one — very nearly regular, exactly tileable.
fn hex_distance(x: f64, v: f64, c: f64) -> f64 {
    let courses = ((c / HEX_ROW) * 0.5).round().max(1.0) * 2.0;
    let y = v * courses * HEX_ROW;
    let row = (v * courses).floor();
    // The nearest centre (the Voronoi cell IS the hexagon) is in this course or a
    // neighbour, within a column either side.
    let mut best = (f64::INFINITY, 0.0, 0.0);
    for j in [row - 1.0, row, row + 1.0] {
        let shift = if j.rem_euclid(2.0) >= 1.0 { 0.5 } else { 0.0 };
        let cy = (j + 0.5) * HEX_ROW;
        let i0 = (x - 0.5 - shift).round();
        for i in [i0 - 1.0, i0, i0 + 1.0] {
            let (dx, dy) = (x - (i + 0.5 + shift), y - cy);
            let d2 = dx * dx + dy * dy;
            if d2 < best.0 {
                best = (d2, dx.abs(), dy.abs());
            }
        }
    }
    let (dx, dy) = (best.1, best.2);
    // The vertical edges' normal, and the slanted edges' at ±60°.
    dx.max(0.5 * dx + HEX_ROW * dy)
}

/// Remap `[lo, hi]` onto `[0, 1]`, then a midtone gamma.
pub(crate) fn levels(h: f64, lo: f64, hi: f64, gamma: f64) -> f64 {
    let span = hi - lo;
    let t = if span.abs() > f64::EPSILON {
        ((h - lo) / span).clamp(0.0, 1.0)
    } else if h >= lo {
        1.0
    } else {
        0.0
    };
    // A gamma of 1 (or a meaningless one ≤ 0) is the identity — skipped, not
    // computed, so a neutral Levels leaves the field bit for bit.
    if gamma > 0.0 && (gamma - 1.0).abs() > f64::EPSILON {
        t.powf(1.0 / gamma)
    } else {
        t
    }
}

/// Push the field away from (or toward) `pivot` — the voice's own shaping,
/// `noise::contrast`, which pivots on the midpoint. Feeding it
/// `h − pivot + ½ + (pivot − ½)/amount` lands its output on
/// `(h − pivot)·amount + pivot`, clamped by the primitive itself, so this node and
/// the rack's `contrast` knob are one formula. A flat contrast (`amount` ≈ 0) cannot
/// be moved that way: it is the pivot itself.
pub(crate) fn contrast(h: f64, pivot: f64, amount: f64) -> f64 {
    if amount.abs() <= f64::EPSILON {
        return pivot.clamp(0.0, 1.0);
    }
    noise::contrast(h - pivot + 0.5 + (pivot - 0.5) / amount, amount)
}

/// A step at `at`, softened into a linear ramp `soft` wide.
pub(crate) fn threshold(h: f64, at: f64, soft: f64) -> f64 {
    if soft > 0.0 {
        ((h - at) / soft + 0.5).clamp(0.0, 1.0)
    } else if h >= at {
        1.0
    } else {
        0.0
    }
}

/// Cross-fade colour `a` toward `b` by `k`, channel by channel, through the rack's
/// own exact-endpoint fade ([`channel::fade`]) — a closed fade is `a`, an open one `b`.
pub(crate) fn mix_color(a: [f32; 3], b: [f32; 3], k: f64) -> [f32; 3] {
    std::array::from_fn(|c| channel::fade(a[c] as f64, b[c] as f64, k) as f32)
}

/// A separable box blur of `radius` (a fraction of the tile), wrapping.
pub(crate) fn blur(src: &[f64], n: u32, radius: f64) -> Vec<f64> {
    let n = n as usize;
    // A window wider than the row would count texels twice.
    let r = in_texels(radius, n).min((n - 1) / 2);
    if r == 0 {
        return src.to_vec();
    }
    let rows = box_pass(src, n, r, |line, k| line * n + k);
    box_pass(&rows, n, r, |line, k| k * n + line)
}

/// One direction of the box: a running window sum along each line, wrapping.
fn box_pass(src: &[f64], n: usize, r: usize, idx: impl Fn(usize, usize) -> usize) -> Vec<f64> {
    let width = (2 * r + 1) as f64;
    let mut out = vec![0.0; src.len()];
    for line in 0..n {
        let tap = |k: usize| src[idx(line, k % n)];
        // The window centred on texel 0 reaches back across the seam.
        let mut sum: f64 = (n - r..n + r + 1).map(tap).sum();
        for k in 0..n {
            out[idx(line, k)] = sum / width;
            sum += tap(k + r + 1) - tap(k + n - r);
        }
    }
    out
}

/// The edge/cavity measure: a texel's lift over the ring `radius` (a fraction of the
/// tile) around it — the occlusion ring's neighbourhood, read signed.
pub(crate) fn curvature(h: &[f32], n: u32, radius: f64) -> Vec<f64> {
    let r = in_texels(radius, n as usize).clamp(1, n as usize) as i64;
    raster_xy(n, |x, y| {
        let lift = bake::at(h, n, x, y) - bake::ring_mean(h, n, x, y, r);
        (0.5 + lift * CURVATURE_GAIN).clamp(0.0, 1.0) as f64
    })
}

/// The baker's ring-cavity occlusion, over a field.
pub(crate) fn occlusion(h: &[f32], n: u32, strength: f32) -> Vec<f64> {
    let r = bake::ring_radius(n);
    raster_xy(n, |x, y| bake::occlusion_at(h, n, x, y, r, strength) as f64)
}

/// Push each texel `amount` of the tile in the direction `by` points, sampling `src`
/// bilinearly there, wrapping.
pub(crate) fn warp(src: &[f64], by: &Plane<f64>, n: u32, amount: f64) -> Vec<f64> {
    let reach = amount * n as f64;
    raster_xy(n, |x, y| {
        let (dy, dx) = (by.at((y * n as i64 + x) as usize) * TAU).sin_cos();
        bilinear(src, n, x as f64 + dx * reach, y as f64 + dy * reach)
    })
}

/// `src` at a fractional texel position, wrapping on the tile.
fn bilinear(src: &[f64], n: u32, sx: f64, sy: f64) -> f64 {
    let side = n as f64;
    // Wrapped first, so the integer corners can never overflow however far the push.
    let (sx, sy) = (sx.rem_euclid(side), sy.rem_euclid(side));
    let (fx, fy) = (sx.floor(), sy.floor());
    let (tx, ty) = (sx - fx, sy - fy);
    let (x0, y0) = (fx as i64, fy as i64);
    let w = n as i64;
    let tap = |x: i64, y: i64| src[(y.rem_euclid(w) * w + x.rem_euclid(w)) as usize];
    let top = tap(x0, y0) + (tap(x0 + 1, y0) - tap(x0, y0)) * tx;
    let bottom = tap(x0, y0 + 1) + (tap(x0 + 1, y0 + 1) - tap(x0, y0 + 1)) * tx;
    top + (bottom - top) * ty
}

/// The input repeated `repeat` times across the tile. Point-sampled, which is EXACT
/// rather than approximate: texel `x` of the repeat sits at tile position
/// `x·repeat/n`, which is precisely input texel `x·repeat mod n`.
pub(crate) fn tile(src: &Plane<f64>, n: u32, repeat: u32) -> Plane<f64> {
    let Plane::Raster(texels) = src else {
        return src.clone();
    };
    let n = n as usize;
    let r = repeat.max(1) as usize % n;
    Plane::Raster(
        (0..n * n)
            .map(|i| {
                let (x, y) = (i % n, i / n);
                texels[(y * r % n) * n + x * r % n]
            })
            .collect(),
    )
}
