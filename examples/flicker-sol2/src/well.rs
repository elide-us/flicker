//! A gravity-well overlay (toggle `G`) — equipotential **contours** of the system's real
//! gravitational potential, plus each planet's **sphere-of-influence** ring.
//!
//! The potential is computed as a field, `Φ(x,y) = −Σ G·mⱼ / √(r² + ε²)`, on a grid, and
//! contoured by marching squares on `ln(−Φ)` (so the levels are log-spaced and bunch into the
//! star's funnel). The SOI ring per planet is the real force crossover with the star
//! (`r_orbit·√(m_planet/M_star)`) — the boundary where a planet out-pulls the star, i.e. the only
//! place a satellite can hold an orbit. Nothing here feeds back into the sim; it just visualises
//! the wells the dynamics already live in.
//!
//! Computing it as a field (not a shortcut) means the day the camera unlocks, the same `Φ` becomes
//! a height and this turns into the rubber-sheet funnel in 3D.

use crate::draw;
use flicker::render::{Renderer, Vec2};

/// Gravitational constant in AU³ / (M☉ · yr²) — `4π²` (matches the sim).
const G: f32 = 39.478_418;
/// Softening² (AU²) for the field — finite at body centres, smooth contours.
const SOFT2: f32 = 0.05 * 0.05;
/// Contour grid resolution (cells per side).
const GRID: usize = 64;
/// Number of equipotential levels.
const LEVELS: usize = 12;

/// Draw the well over the view. `bodies` are `(position AU, mass M☉)` with the star first;
/// `center`/`px_per_au` map AU → screen; `half_au` is half the grid width in AU (cover the view).
pub fn draw(r: &mut Renderer, center: Vec2, px_per_au: f32, half_au: f32, bodies: &[(Vec2, f32)]) {
    if bodies.is_empty() || half_au <= 0.0 {
        return;
    }
    let n = GRID;
    let step = 2.0 * half_au / n as f32;

    // Depth field d = ln(−Φ) at each grid node.
    let mut d = vec![0f32; (n + 1) * (n + 1)];
    let (mut dmin, mut dmax) = (f32::MAX, f32::MIN);
    for gy in 0..=n {
        for gx in 0..=n {
            let x = -half_au + gx as f32 * step;
            let y = -half_au + gy as f32 * step;
            let mut pot = 0.0f32;
            for &(bp, bm) in bodies {
                let dx = x - bp.x;
                let dy = y - bp.y;
                pot += bm / (dx * dx + dy * dy + SOFT2).sqrt();
            }
            let v = (G * pot).max(1e-6).ln();
            d[gy * (n + 1) + gx] = v;
            dmin = dmin.min(v);
            dmax = dmax.max(v);
        }
    }
    if dmax <= dmin {
        return;
    }

    let to_px = |x: f32, y: f32| Vec2::new(center.x + x * px_per_au, center.y + y * px_per_au);
    let contour = [0.40, 0.62, 0.85, 0.22];

    // Marching squares per level.
    for li in 1..LEVELS {
        let level = dmin + (dmax - dmin) * li as f32 / LEVELS as f32;
        for gy in 0..n {
            for gx in 0..n {
                let i = gy * (n + 1) + gx;
                let (v0, v1, v2, v3) = (d[i], d[i + 1], d[i + n + 2], d[i + n + 1]);
                let x0 = -half_au + gx as f32 * step;
                let y0 = -half_au + gy as f32 * step;
                let mut pts: [Vec2; 4] = [Vec2::ZERO; 4];
                let mut k = 0;
                if (v0 < level) != (v1 < level) {
                    pts[k] = to_px(x0 + (level - v0) / (v1 - v0) * step, y0);
                    k += 1;
                }
                if (v1 < level) != (v2 < level) {
                    pts[k] = to_px(x0 + step, y0 + (level - v1) / (v2 - v1) * step);
                    k += 1;
                }
                if (v2 < level) != (v3 < level) {
                    pts[k] = to_px(x0 + step - (level - v2) / (v3 - v2) * step, y0 + step);
                    k += 1;
                }
                if (v3 < level) != (v0 < level) {
                    pts[k] = to_px(x0, y0 + step - (level - v3) / (v0 - v3) * step);
                    k += 1;
                }
                if k >= 2 {
                    draw::line(r, pts[0], pts[1], 1.0, contour);
                    if k == 4 {
                        draw::line(r, pts[2], pts[3], 1.0, contour);
                    }
                }
            }
        }
    }

    // Each planet's sphere of influence — the real force crossover with the star (body 0).
    let star_m = bodies[0].1;
    let soi = [0.55, 0.78, 0.95, 0.45];
    for &(bp, bm) in &bodies[1..] {
        if bm < 1e-6 || star_m <= 0.0 {
            continue;
        }
        let rad_px = bp.length() * (bm / star_m).sqrt() * px_per_au;
        if !(3.0..=half_au * px_per_au).contains(&rad_px) {
            continue;
        }
        let segs = ((rad_px / 3.0) as usize).clamp(16, 64);
        draw::ring(r, to_px(bp.x, bp.y), rad_px, 1.0, soi, segs);
    }
}
