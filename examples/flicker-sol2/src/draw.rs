//! Tiny 2D drawing helpers built on the renderer's pixel-space `draw_triangle`.
//!
//! `flicker-render` has no native circle / line primitive for the 2D layer, so we
//! tessellate: a ring is an annulus of quads, a line is a thin quad, an arrow adds a
//! triangular head. All take pixel coordinates (origin top-left) and RGBA colours
//! with alpha — the 2D pipeline alpha-blends in painter's order, so translucent
//! rings layer cleanly.

use std::f32::consts::TAU;

use flicker::render::{Renderer, Vec2};

/// A filled disc (triangle fan) of `segs` segments.
pub fn disc(r: &mut Renderer, center: Vec2, radius: f32, color: [f32; 4], segs: usize) {
    let mut prev = Vec2::new(center.x + radius, center.y);
    for i in 1..=segs {
        let a = i as f32 / segs as f32 * TAU;
        let p = Vec2::new(center.x + radius * a.cos(), center.y + radius * a.sin());
        r.draw_triangle(center, prev, p, color);
        prev = p;
    }
}

/// A ring (annulus) of mean `radius` and given `thickness`, as `segs` quads.
pub fn ring(r: &mut Renderer, center: Vec2, radius: f32, thickness: f32, color: [f32; 4], segs: usize) {
    let ro = radius + thickness * 0.5;
    let ri = (radius - thickness * 0.5).max(0.0);
    let pt = |rad: f32, a: f32| Vec2::new(center.x + rad * a.cos(), center.y + rad * a.sin());
    for i in 0..segs {
        let a0 = i as f32 / segs as f32 * TAU;
        let a1 = (i + 1) as f32 / segs as f32 * TAU;
        let o0 = pt(ro, a0);
        let o1 = pt(ro, a1);
        let i0 = pt(ri, a0);
        let i1 = pt(ri, a1);
        r.draw_triangle(o0, o1, i1, color);
        r.draw_triangle(o0, i1, i0, color);
    }
}

/// A ring whose **radius and colour vary with angle** — for lumpy, meandering rings.
/// `radius_at(theta)` gives the centre-line radius and `color_at(theta)` the RGBA at
/// each segment, both sampled around the circle. `thickness` is constant.
pub fn ring_sampled<R, C>(
    renderer: &mut Renderer,
    center: Vec2,
    thickness: f32,
    segs: usize,
    mut radius_at: R,
    mut color_at: C,
) where
    R: FnMut(f32) -> f32,
    C: FnMut(f32) -> [f32; 4],
{
    let half = thickness * 0.5;
    let pt = |rad: f32, a: f32| Vec2::new(center.x + rad * a.cos(), center.y + rad * a.sin());
    let mut a0 = 0.0_f32;
    let mut rad0 = radius_at(a0);
    for i in 0..segs {
        let a1 = (i + 1) as f32 / segs as f32 * TAU;
        let rad1 = radius_at(a1);
        let col = color_at((a0 + a1) * 0.5);
        let o0 = pt(rad0 + half, a0);
        let o1 = pt(rad1 + half, a1);
        let i1 = pt((rad1 - half).max(0.0), a1);
        let i0 = pt((rad0 - half).max(0.0), a0);
        renderer.draw_triangle(o0, o1, i1, col);
        renderer.draw_triangle(o0, i1, i0, col);
        a0 = a1;
        rad0 = rad1;
    }
}

/// A straight line segment of the given `thickness` (a thin quad).
pub fn line(r: &mut Renderer, a: Vec2, b: Vec2, thickness: f32, color: [f32; 4]) {
    let d = b - a;
    let len = d.length();
    if len < 1e-4 {
        return;
    }
    let n = Vec2::new(-d.y, d.x) / len * (thickness * 0.5);
    r.draw_triangle(a + n, a - n, b - n, color);
    r.draw_triangle(a + n, b - n, b + n, color);
}

/// A dotted line from `a` to `b` — dashes `dash` px long separated by `gap` px.
pub fn dotted(r: &mut Renderer, a: Vec2, b: Vec2, thickness: f32, color: [f32; 4], dash: f32, gap: f32) {
    let d = b - a;
    let len = d.length();
    if len < 1e-4 {
        return;
    }
    let dir = d / len;
    let mut t = 0.0;
    while t < len {
        let s = a + dir * t;
        let e = a + dir * (t + dash).min(len);
        line(r, s, e, thickness, color);
        t += dash + gap;
    }
}

/// An arrow from `from` to `to`: a line shaft plus a triangular head of size `head`.
pub fn arrow(r: &mut Renderer, from: Vec2, to: Vec2, thickness: f32, head: f32, color: [f32; 4]) {
    let d = to - from;
    let len = d.length();
    if len < 1e-4 {
        return;
    }
    let dir = d / len;
    line(r, from, to - dir * (head * 0.5), thickness, color);
    let perp = Vec2::new(-dir.y, dir.x);
    let base = to - dir * head;
    r.draw_triangle(to, base + perp * (head * 0.5), base - perp * (head * 0.5), color);
}
