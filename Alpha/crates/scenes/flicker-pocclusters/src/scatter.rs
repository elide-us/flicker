//! Grass SCATTER placement — pure, GPU-free, coordinate-convention-free. It decides WHERE each
//! grass instance stands: candidate points on a jittered grid over the terrain area, the ground
//! height sampled from the terrain, kept only ABOVE the waterline, each given a weighted-random
//! variant + a yaw + a uniform scale. The scene turns each [`GrassPlacement`] into a draw matrix
//! and mesh handle (the up-axis / world basis is the scene's business, kept out of here).
//!
//! Deterministic: every random value is a hash of the grid cell + a fixed seed, so the field is
//! identical every frame and every run with no stored RNG — the reproducible-placement pattern the
//! renderer's per-cell `hash01` uses.

/// Where one grass instance stands. `pos` is `[x, y, ground_height]` in the terrain's world space;
/// `variant` indexes the caller's parallel list of set variants / uploaded mesh handles.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GrassPlacement {
    pub pos: [f32; 3],
    /// Radians about the world up-axis.
    pub yaw: f32,
    /// Uniform scale multiplier applied to the (real-cm) variant mesh.
    pub scale: f32,
    pub variant: usize,
}

/// Inputs to [`scatter`]. Lengths are in the terrain's world units (centimetres, matching the
/// baked props and the heightfield).
#[derive(Clone, Copy, Debug)]
pub struct ScatterParams {
    /// World-XY bounds of the area to populate.
    pub area_min: [f32; 2],
    pub area_max: [f32; 2],
    /// Grid step — the mean spacing between blades before jitter.
    pub spacing: f32,
    /// Positional jitter as a fraction `0..1` of one cell (0 = perfect grid, 1 = up to ±half a cell).
    pub jitter: f32,
    /// Terrain height of the water surface; a candidate is dropped when its ground is at/below it.
    pub sea_level: f32,
    /// Keep grass this far ABOVE `sea_level` so it never stands in the surf.
    pub shore_margin: f32,
    /// Uniform per-instance scale jitter range.
    pub scale_min: f32,
    pub scale_max: f32,
    /// Seed — vary the whole field without moving the area.
    pub seed: u32,
}

/// Deterministic Wang-style hash of `x` into `[0, 1)`.
fn hash01(mut x: u32) -> f32 {
    x = (x ^ 61) ^ (x >> 16);
    x = x.wrapping_add(x << 3);
    x ^= x >> 4;
    x = x.wrapping_mul(0x27d4_eb2d);
    x ^= x >> 15;
    (x & 0x00FF_FFFF) as f32 / 16_777_216.0
}

/// Weighted pick: `r` in `[0, 1)` → an index into `weights` proportional to weight (the same walk
/// as `flicker_content::PropSet::pick`, inlined so this module stays dependency-free).
fn pick_weighted(weights: &[f32], r: f32) -> usize {
    let total: f32 = weights.iter().sum();
    if total <= 0.0 {
        return 0;
    }
    let mut acc = total * r.clamp(0.0, 1.0);
    for (i, w) in weights.iter().enumerate() {
        acc -= w;
        if acc < 0.0 {
            return i;
        }
    }
    weights.len().saturating_sub(1)
}

/// Populate the area with grass placements. `weights[i]` is the spawn weight of variant `i`;
/// `height_at(x, y)` returns the terrain ground height at a world XY (the scene wraps the
/// heightfield). Returns one [`GrassPlacement`] per kept candidate.
pub fn scatter(
    weights: &[f32],
    p: &ScatterParams,
    height_at: impl Fn(f32, f32) -> f32,
) -> Vec<GrassPlacement> {
    let mut out = Vec::new();
    if p.spacing <= 0.0 || weights.is_empty() {
        return out;
    }
    let nx = (((p.area_max[0] - p.area_min[0]) / p.spacing).floor() as i64).max(0);
    let ny = (((p.area_max[1] - p.area_min[1]) / p.spacing).floor() as i64).max(0);
    for gy in 0..ny {
        for gx in 0..nx {
            let cell = p
                .seed
                .wrapping_add((gx as u32).wrapping_mul(0x9E37_79B9))
                .wrapping_add((gy as u32).wrapping_mul(0x85EB_CA6B));
            let jx = (hash01(cell ^ 0x1111_1111) - 0.5) * p.jitter * p.spacing;
            let jy = (hash01(cell ^ 0x2222_2222) - 0.5) * p.jitter * p.spacing;
            let x = p.area_min[0] + (gx as f32 + 0.5) * p.spacing + jx;
            let y = p.area_min[1] + (gy as f32 + 0.5) * p.spacing + jy;
            let h = height_at(x, y);
            if h <= p.sea_level + p.shore_margin {
                continue; // in the water (or the surf margin) — no grass
            }
            let variant = pick_weighted(weights, hash01(cell ^ 0x3333_3333));
            let yaw = hash01(cell ^ 0x4444_4444) * std::f32::consts::TAU;
            let scale = p.scale_min + hash01(cell ^ 0x5555_5555) * (p.scale_max - p.scale_min);
            out.push(GrassPlacement {
                pos: [x, y, h],
                yaw,
                scale,
                variant,
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params() -> ScatterParams {
        ScatterParams {
            area_min: [-1000.0, -1000.0],
            area_max: [1000.0, 1000.0],
            spacing: 100.0,
            jitter: 0.6,
            sea_level: 120.0,
            shore_margin: 5.0,
            scale_min: 0.8,
            scale_max: 1.2,
            seed: 1,
        }
    }

    /// Half the terrain is under water (x < 0 → below sea level); every placement must be on the
    /// dry half AND above the waterline, and the field must be non-empty.
    #[test]
    fn only_places_above_the_waterline() {
        let p = params();
        // West half drowned, east half a 500 cm plateau.
        let height = |x: f32, _y: f32| if x < 0.0 { -50.0 } else { 500.0 };
        let g = scatter(&[1.0, 1.0, 2.0], &p, height);
        assert!(!g.is_empty(), "the dry half is populated");
        for pl in &g {
            assert!(
                pl.pos[2] > p.sea_level + p.shore_margin,
                "placed above the waterline (z={})",
                pl.pos[2]
            );
            assert!(pl.pos[0] >= 0.0, "no grass on the drowned half (x={})", pl.pos[0]);
            assert!((0.8..=1.2).contains(&pl.scale));
            assert!((0..3).contains(&pl.variant));
        }
    }

    /// A fully drowned terrain yields no grass; degenerate params yield none either.
    #[test]
    fn drowned_or_degenerate_is_empty() {
        let p = params();
        assert!(scatter(&[1.0], &p, |_, _| -100.0).is_empty(), "all underwater");
        let mut bad = p;
        bad.spacing = 0.0;
        assert!(scatter(&[1.0], &bad, |_, _| 500.0).is_empty(), "no spacing");
        assert!(scatter(&[], &p, |_, _| 500.0).is_empty(), "no variants");
    }

    /// Fully deterministic: identical inputs reproduce the field byte-for-byte.
    #[test]
    fn is_deterministic() {
        let p = params();
        let a = scatter(&[1.0, 2.0], &p, |_, _| 500.0);
        let b = scatter(&[1.0, 2.0], &p, |_, _| 500.0);
        assert_eq!(a, b);
        assert!(!a.is_empty());
    }

    /// The weighted pick tracks the weights across the field (variant 2 has half the mass).
    #[test]
    fn variant_mix_tracks_weights() {
        let p = params();
        let g = scatter(&[1.0, 1.0, 2.0], &p, |_, _| 500.0);
        let n = g.len() as f32;
        let heavy = g.iter().filter(|pl| pl.variant == 2).count() as f32;
        assert!(
            (heavy / n - 0.5).abs() < 0.08,
            "variant 2 ~ 50% of the field, got {}",
            heavy / n
        );
    }
}
