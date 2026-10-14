//! **THE FIELD RECIPE — the V5 stack as DATA, composed the one way.**
//!
//! What the WorldBench's dials hold, as a record: every field's seed and dials,
//! the sea dial, the map size. [`compose`] turns a recipe into the composed
//! HEIGHT channel and the sea — the same arithmetic the bench draws, stated
//! once here so a headless evaluator and the window cannot disagree (the bench
//! carries a gate that its heights equal `compose(bench.recipe())`).
//!
//! On top of that, the two things a shape iteration loop needs without a GPU:
//! - [`metrics`] — MEASURED properties of the composed shape (land share,
//!   continents, coastline, where the ridges stand, the ocean floor's relief),
//!   so a dial sweep can be scored against Aaron's targets (seven continents on
//!   ~30% land, ridges on continental edges, basins and seabeds, ripples);
//! - [`render_equirect`] — the sphere DE-PROJECTED to a flat map with
//!   hypsometric tints and elevation contour rings, the civil-engineering
//!   drawing a reviewer (human or agent) reads the shape from.
//!
//! None of this is simulation: the recipe is authored data, the composition a
//! pure function, the metrics a reading. The transformations-not-outcomes law
//! (935269B7) binds the FIELDS; a recipe is the dials a game master turns.

use glam::Vec3;
use serde::{Deserialize, Serialize};

use crate::crust::CrustField;
use crate::map::{HexMap, TileId};
use crate::resonance::{
    land_share, sea_level, HarmonicWave, PlanetWave, VolcanoShape, Volcanoes, WaveMask,
    DEFAULT_AXES, DEFAULT_LOBE,
};
use crate::seams::{SeamField, DEFAULT_CELLS, DEFAULT_SPOTS};

/// A field's alpha mask: its roll and whether it applies.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct MaskRecipe {
    pub seed: u64,
    pub on: bool,
}

/// The planetary wave's record.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct WaveRecipe {
    pub seed: u64,
    /// Relief as a fraction of the radius.
    pub amp: f32,
    pub lobe: f32,
    /// **The plateau exponent**: the wave's normalized value is reshaped as
    /// `sign(v)·|v|^plateau` before it scales. 1 leaves the domes as domes;
    /// below 1 lifts the mid-slopes into flat-topped continents with steep
    /// flanks (and deepens the basins to flat floors); above 1 sharpens the
    /// crests into peaks.
    pub plateau: f32,
    pub mask: MaskRecipe,
    pub on: bool,
    /// The COAST RIDGE — the wave's child: a band of relief that follows the
    /// wave's own contour at `level` (its value, −1..1), `width` wide in value,
    /// `amp` high — mountain ridges along the continents' edges, riding the
    /// wave's mask so a muted region grows no ridge.
    pub ridge: RidgeRecipe,
}

/// A ridge band on a parent field's contour.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct RidgeRecipe {
    /// The parent's value the ridge crest follows (−1..1).
    pub level: f32,
    /// The band's half-width in parent value.
    pub width: f32,
    /// Ridge height, radius units.
    pub amp: f32,
    pub on: bool,
}

/// Reshape a normalized wave value by the plateau exponent.
pub fn plateau(v: f32, exponent: f32) -> f32 {
    v.signum() * v.abs().powf(exponent.max(0.05))
}

/// The ridge band's profile at parent value `v`.
pub fn ridge(v: f32, r: &RidgeRecipe) -> f32 {
    let x = (v - r.level) / r.width.max(1e-3);
    r.amp * (-x * x).exp()
}

/// One harmonic's record.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct HarmonicRecipe {
    pub seed: u64,
    pub ratio: f32,
    pub axes: u32,
    /// Relief as a fraction of the radius.
    pub amp: f32,
    pub mask: MaskRecipe,
    pub on: bool,
}

/// The heat field's record (the molten seams), and the emitters stamped on
/// its vents.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct HeatRecipe {
    pub seed: u64,
    pub cells: u32,
    pub spots: u32,
    pub on: bool,
    pub emitters: VolcanoShape,
    pub emitters_on: bool,
}

/// **The COAST BAND** — the first BAND field of the proposed stack: an ALTITUDE
/// belt over the composed relief. Where the composed height stands `level`
/// above the sea (± `width`) it adds `amp` — mountain ridges along whatever
/// the coast turned out to be, after every wave had its say. Reads the
/// composition, so it is applied LAST; it is a band, not a wave, and has no
/// roll of its own.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct CoastBandRecipe {
    /// Height above the sea the crest follows, radius units.
    pub level: f32,
    /// The belt's half-width, radius units.
    pub width: f32,
    /// Ridge height, radius units.
    pub amp: f32,
    pub on: bool,
}

/// **The whole stack.**
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Recipe {
    pub freq: u32,
    pub wave: WaveRecipe,
    pub harmonics: Vec<HarmonicRecipe>,
    pub heat: HeatRecipe,
    pub coast: CoastBandRecipe,
    /// The sea dial, percent.
    pub sea_percent: f32,
}

impl Recipe {
    /// The WorldBench's opening recipe at `seed`: every field's roll derived
    /// from the one seed (so a sweep over seeds is a sweep over worlds), the
    /// dials at the bench's defaults — the planetary wave, the 8/9, 1/13 and
    /// 1/15 harmonics, the heat and its emitters, the sea at the mean.
    pub fn default_at(freq: u32, seed: u64) -> Self {
        let sub = |k: u64| {
            seed.wrapping_mul(0x9E37_79B9_7F4A_7C15)
                .wrapping_add(k * 0x2545_F491_4F6C_DD1D)
        };
        let harmonic = |k: u64, ratio: f32, amp: f32| HarmonicRecipe {
            seed: sub(k),
            ratio,
            axes: DEFAULT_AXES,
            amp,
            mask: MaskRecipe { seed: 1, on: true },
            on: true,
        };
        Self {
            freq,
            wave: WaveRecipe {
                seed: sub(1),
                amp: 0.04,
                lobe: DEFAULT_LOBE,
                plateau: 1.0,
                mask: MaskRecipe { seed: 1, on: true },
                on: true,
                ridge: RidgeRecipe {
                    level: 0.15,
                    width: 0.12,
                    amp: 0.02,
                    on: false,
                },
            },
            harmonics: vec![
                harmonic(2, 8.0 / 9.0, 0.020),
                harmonic(3, 13.0, 0.006),
                harmonic(4, 15.0, 0.005),
            ],
            heat: HeatRecipe {
                seed: sub(5),
                cells: DEFAULT_CELLS,
                spots: DEFAULT_SPOTS,
                on: true,
                emitters: VolcanoShape {
                    amp: 0.005,
                    ..VolcanoShape::default()
                },
                emitters_on: true,
            },
            // The bench opens with the coast band ON at the sweep-found belt
            // (2026-10-13: ridges along whatever the coast turned out to be).
            coast: CoastBandRecipe {
                level: 0.004,
                width: 0.003,
                amp: 0.03,
                on: true,
            },
            sea_percent: 49.0,
        }
    }
}

/// The composed shape: the height channel per tile (radius units) and the
/// standing sea (the base wave's datum).
pub struct Composed {
    pub heights: Vec<f32>,
    pub sea: f32,
}

/// **Compose a recipe** over its map: `wave·amp·mask + Σ harmonics + emitters`,
/// add being the op; the sea solved over the BASE wave's own span alone.
pub fn compose(map: &HexMap, r: &Recipe) -> Composed {
    let n = map.len();
    let mut heights = vec![0.0f32; n];
    let mut sea_span = (0.0f32, 0.0f32);
    if r.wave.on {
        let wave = PlanetWave::with_lobe(map, r.wave.seed, r.wave.lobe);
        let mask = WaveMask::new(map, r.wave.mask.seed);
        sea_span = wave
            .values()
            .iter()
            .fold((f32::MAX, f32::MIN), |(lo, hi), v| {
                (lo.min(v * r.wave.amp), hi.max(v * r.wave.amp))
            });
        for (i, h) in heights.iter_mut().enumerate() {
            let m = if r.wave.mask.on {
                mask.value(i as TileId)
            } else {
                1.0
            };
            let v = plateau(wave.value(i as TileId), r.wave.plateau);
            *h += v * r.wave.amp * m;
            if r.wave.ridge.on {
                *h += ridge(v, &r.wave.ridge) * m;
            }
        }
    }
    for hr in r.harmonics.iter().filter(|h| h.on) {
        let wave = HarmonicWave::new(map, hr.seed, hr.ratio, hr.axes);
        let mask = WaveMask::new(map, hr.mask.seed);
        for (i, h) in heights.iter_mut().enumerate() {
            let m = if hr.mask.on {
                mask.value(i as TileId)
            } else {
                1.0
            };
            *h += wave.value(i as TileId) * hr.amp * m;
        }
    }
    if r.heat.on && r.heat.emitters_on {
        let seams = SeamField::new(map, r.heat.cells, r.heat.spots, r.heat.seed);
        let crust = CrustField::derive(map, &seams);
        let volcano = Volcanoes::new(map, &seams, crust.vents(), r.heat.emitters);
        for (i, h) in heights.iter_mut().enumerate() {
            *h += volcano.value(i as TileId);
        }
    }
    let sea = sea_level(sea_span.0, sea_span.1, r.sea_percent);
    if r.coast.on {
        // The altitude belt over the composition: ridges along the coast as it
        // stands after every wave. Reads the pre-band heights (one pass, no
        // self-reference).
        // LAND ONLY and truncated: the belt never reaches below the waterline
        // (a tail over the shallows would raise the sea floor into land) and
        // ends cleanly at 2.5 widths.
        let pre = heights.clone();
        for (h, p) in heights.iter_mut().zip(&pre) {
            let rel = p - sea;
            if rel <= 0.0 {
                continue; // strictly LAND: the waterline itself grows no ridge
            }
            let x = (rel - r.coast.level) / r.coast.width.max(1e-6);
            if x.abs() > 2.5 {
                continue;
            }
            *h += r.coast.amp * (-x * x).exp();
        }
    }
    Composed { heights, sea }
}

/// A land mass smaller than this share of the surface is an island, not a
/// continent (Australia ≈ 1.5% of Earth's surface).
pub const CONTINENT_MIN_SHARE: f32 = 0.015;
/// How many tiles inland "the edge of a continent" reaches for the ridge
/// reading (~150 mi at the standard tile).
pub const EDGE_RINGS: u32 = 3;

/// **What the composed shape measures as** — the readings a sweep is scored by.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Metrics {
    /// Area share of the surface above the sea.
    pub land_share: f32,
    /// Land masses at or above [`CONTINENT_MIN_SHARE`] of the surface.
    pub continents: usize,
    /// Land masses below it.
    pub islands: usize,
    /// The largest land mass's share of ALL land (1.0 = one supercontinent).
    pub largest_land_share: f32,
    /// Coast tiles (land with a sea neighbour) per land tile — the coastline's
    /// crookedness: a round blob reads low, a ragged coast high.
    pub coast_per_land: f32,
    /// Share of the highest-decile land tiles that stand within [`EDGE_RINGS`]
    /// of a coast — "mountain ridges on the edge of continents".
    pub ridges_on_edges: f32,
    /// Mean land height above the sea, radius units.
    pub mean_land_height: f32,
    /// The ocean floor's relief: the standard deviation of sea-tile depths over
    /// the mean depth — basins and seabeds vs a flat floor.
    pub seabed_relief: f32,
    /// Sea-floor basins: connected sea regions deeper than the median depth, at
    /// or above [`CONTINENT_MIN_SHARE`] of the surface.
    pub basins: usize,
}

/// Measure a composed shape.
pub fn metrics(map: &HexMap, c: &Composed) -> Metrics {
    let n = map.len();
    let area = &map.grid().area;
    let total_area: f32 = area.iter().sum();
    let land: Vec<bool> = c.heights.iter().map(|h| *h > c.sea).collect();
    let land_share = land_share(map, |t| c.heights[t as usize], c.sea);

    // Connected land masses (BFS), area-weighted.
    let components = |keep: &dyn Fn(usize) -> bool| -> Vec<f32> {
        let mut seen = vec![false; n];
        let mut out = Vec::new();
        for s in 0..n {
            if seen[s] || !keep(s) {
                continue;
            }
            let mut stack = vec![s];
            seen[s] = true;
            let mut a = 0.0f32;
            while let Some(t) = stack.pop() {
                a += area[t];
                for nb in map.neighbours(t as TileId) {
                    let j = *nb as usize;
                    if !seen[j] && keep(j) {
                        seen[j] = true;
                        stack.push(j);
                    }
                }
            }
            out.push(a / total_area);
        }
        out
    };
    let masses = components(&|t| land[t]);
    let continents = masses.iter().filter(|a| **a >= CONTINENT_MIN_SHARE).count();
    let islands = masses.len() - continents;
    let largest = masses.iter().copied().fold(0.0f32, f32::max);
    let largest_land_share = if land_share > 0.0 {
        largest / land_share
    } else {
        0.0
    };

    // Coast: land with a sea neighbour; distance-to-coast over land by BFS.
    let mut coast_dist = vec![u32::MAX; n];
    let mut frontier: Vec<usize> = Vec::new();
    let mut coast_tiles = 0usize;
    let mut land_tiles = 0usize;
    for t in 0..n {
        if !land[t] {
            continue;
        }
        land_tiles += 1;
        if map
            .neighbours(t as TileId)
            .iter()
            .any(|nb| !land[*nb as usize])
        {
            coast_dist[t] = 0;
            frontier.push(t);
            coast_tiles += 1;
        }
    }
    let mut d = 0u32;
    while !frontier.is_empty() {
        let mut next = Vec::new();
        for t in &frontier {
            for nb in map.neighbours(*t as TileId) {
                let j = *nb as usize;
                if land[j] && coast_dist[j] == u32::MAX {
                    coast_dist[j] = d + 1;
                    next.push(j);
                }
            }
        }
        frontier = next;
        d += 1;
    }
    let coast_per_land = if land_tiles > 0 {
        coast_tiles as f32 / land_tiles as f32
    } else {
        0.0
    };
    // The highest decile of land, and where it stands.
    let mut land_h: Vec<f32> = (0..n)
        .filter(|t| land[*t])
        .map(|t| c.heights[t] - c.sea)
        .collect();
    land_h.sort_by(f32::total_cmp);
    let (ridges_on_edges, mean_land_height) = if land_h.is_empty() {
        (0.0, 0.0)
    } else {
        let p90 = land_h[land_h.len() * 9 / 10];
        let (mut high, mut near) = (0usize, 0usize);
        for t in 0..n {
            if land[t] && c.heights[t] - c.sea >= p90 {
                high += 1;
                if coast_dist[t] <= EDGE_RINGS {
                    near += 1;
                }
            }
        }
        (
            if high > 0 {
                near as f32 / high as f32
            } else {
                0.0
            },
            land_h.iter().sum::<f32>() / land_h.len() as f32,
        )
    };
    // The sea floor.
    let depths: Vec<f32> = (0..n)
        .filter(|t| !land[*t])
        .map(|t| c.sea - c.heights[t])
        .collect();
    let (seabed_relief, basins) = if depths.len() < 2 {
        (0.0, 0)
    } else {
        let mean = depths.iter().sum::<f32>() / depths.len() as f32;
        let var = depths.iter().map(|x| (x - mean).powi(2)).sum::<f32>() / depths.len() as f32;
        let mut sorted = depths.clone();
        sorted.sort_by(f32::total_cmp);
        let median = sorted[sorted.len() / 2];
        let deep = components(&|t| !land[t] && c.sea - c.heights[t] > median);
        (
            if mean > 0.0 { var.sqrt() / mean } else { 0.0 },
            deep.iter().filter(|a| **a >= CONTINENT_MIN_SHARE).count(),
        )
    };
    Metrics {
        land_share,
        continents,
        islands,
        largest_land_share,
        coast_per_land,
        ridges_on_edges,
        mean_land_height,
        seabed_relief,
        basins,
    }
}

/// **The targets** a sweep scores against (Aaron 2026-10-13): seven continents
/// on ~30% land, ridges on the continents' edges, basins and seabeds, coasts
/// with variety. Lower is better; zero is every target met.
pub fn score(m: &Metrics) -> f32 {
    let land = (m.land_share - 0.30).abs() / 0.10;
    let conts = (m.continents as f32 - 7.0).abs() / 3.0;
    let split = (m.largest_land_share - 0.3).max(0.0) / 0.3; // no supercontinent
    let ridges = (0.6 - m.ridges_on_edges).max(0.0) / 0.6;
    let coast = (0.25 - m.coast_per_land).max(0.0) / 0.25;
    let floor = (0.35 - m.seabed_relief).max(0.0) / 0.35;
    let basins = (3.0 - m.basins as f32).max(0.0) / 3.0;
    2.0 * land + 2.0 * conts + split + ridges + 0.5 * coast + 0.5 * floor + 0.5 * basins
}

/// **De-project the sphere**: an equirectangular RGB image, `w × h`, the sea in
/// depth-shaded blue, the land in a hypsometric ramp (lowland green → upland
/// brown → summit white), and ELEVATION CONTOUR RINGS every `contour` radius
/// units drawn dark where a pixel's band differs from its neighbour's — the
/// civil-engineering drawing of the shape. Pixels sample the nearest tile
/// through a lat/lon bucket grid.
pub fn render_equirect(map: &HexMap, c: &Composed, w: u32, h: u32, contour: f32) -> Vec<u8> {
    let dirs = &map.grid().dirs;
    // Bucket the tiles by 2° cells of (lat, lon) for a fast nearest lookup.
    const CELL_DEG: f32 = 2.0;
    let (bw, bh) = ((360.0 / CELL_DEG) as usize, (180.0 / CELL_DEG) as usize);
    let mut buckets: Vec<Vec<u32>> = vec![Vec::new(); bw * bh];
    let bucket_of = |d: Vec3| -> (usize, usize) {
        let lat = d.y.clamp(-1.0, 1.0).asin().to_degrees();
        let lon = d.z.atan2(d.x).to_degrees();
        let bx = (((lon + 180.0) / CELL_DEG) as usize).min(bw - 1);
        let by = (((lat + 90.0) / CELL_DEG) as usize).min(bh - 1);
        (bx, by)
    };
    for (i, d) in dirs.iter().enumerate() {
        let (bx, by) = bucket_of(*d);
        buckets[by * bw + bx].push(i as u32);
    }
    let nearest = |d: Vec3| -> usize {
        let (bx, by) = bucket_of(d);
        let mut best = (f32::MIN, 0usize);
        for dy in -1i32..=1 {
            let yy = by as i32 + dy;
            if yy < 0 || yy >= bh as i32 {
                continue;
            }
            for dx in -1i32..=1 {
                let xx = (bx as i32 + dx).rem_euclid(bw as i32);
                for &t in &buckets[yy as usize * bw + xx as usize] {
                    let dot = dirs[t as usize].dot(d);
                    if dot > best.0 {
                        best = (dot, t as usize);
                    }
                }
            }
        }
        best.1
    };
    // Height per pixel first (contours need neighbours).
    let (lo, hi) = c
        .heights
        .iter()
        .fold((f32::MAX, f32::MIN), |(lo, hi), v| (lo.min(*v), hi.max(*v)));
    let mut hp = vec![0.0f32; (w * h) as usize];
    for y in 0..h {
        let lat = (0.5 - (y as f32 + 0.5) / h as f32) * std::f32::consts::PI;
        for x in 0..w {
            let lon = ((x as f32 + 0.5) / w as f32 - 0.5) * std::f32::consts::TAU;
            let d = Vec3::new(lat.cos() * lon.cos(), lat.sin(), lat.cos() * lon.sin());
            hp[(y * w + x) as usize] = c.heights[nearest(d)];
        }
    }
    let band = |v: f32| ((v - c.sea) / contour.max(1e-6)).floor() as i32;
    let mut rgb = vec![0u8; (w * h * 3) as usize];
    for y in 0..h {
        for x in 0..w {
            let i = (y * w + x) as usize;
            let v = hp[i];
            let mut col = if v <= c.sea {
                // Depth-shaded blue: shallow pale → deep dark.
                let depth = ((c.sea - v) / (c.sea - lo).max(1e-6)).clamp(0.0, 1.0);
                [
                    (0.45 - 0.40 * depth) * 255.0,
                    (0.62 - 0.50 * depth) * 255.0,
                    (0.85 - 0.45 * depth) * 255.0,
                ]
            } else {
                let t = ((v - c.sea) / (hi - c.sea).max(1e-6)).clamp(0.0, 1.0);
                let (a, b, f) = if t < 0.5 {
                    ([0.36, 0.56, 0.28], [0.58, 0.47, 0.30], t * 2.0)
                } else {
                    ([0.58, 0.47, 0.30], [0.96, 0.96, 0.96], (t - 0.5) * 2.0)
                };
                [
                    (a[0] + (b[0] - a[0]) * f) * 255.0,
                    (a[1] + (b[1] - a[1]) * f) * 255.0,
                    (a[2] + (b[2] - a[2]) * f) * 255.0,
                ]
            };
            // Contour ring: this pixel's band differs from the one to its right
            // or below. The shoreline (band 0 ↔ −1) draws darkest.
            let b0 = band(v);
            let right = if x + 1 < w { band(hp[i + 1]) } else { b0 };
            let below = if y + 1 < h {
                band(hp[i + w as usize])
            } else {
                b0
            };
            if b0 != right || b0 != below {
                let k = if (b0 >= 0) != (right >= 0) || (b0 >= 0) != (below >= 0) {
                    0.25
                } else {
                    0.6
                };
                col = [col[0] * k, col[1] * k, col[2] * k];
            }
            rgb[i * 3] = col[0] as u8;
            rgb[i * 3 + 1] = col[1] as u8;
            rgb[i * 3 + 2] = col[2] as u8;
        }
    }
    rgb
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::map::MIN_FREQ;

    /// **A recipe composes the one way, and the readings read what is there.**
    /// Same recipe, same shape (serialized and back too); every field off is a
    /// flat sphere with no land at any sea above the base; the default recipe
    /// lands between all-land and all-sea with at least one land mass, and its
    /// flat render is the right size with sea and land pixels both present.
    #[test]
    fn a_recipe_composes_deterministically_and_measures() {
        let map = HexMap::new(MIN_FREQ);
        let r = Recipe::default_at(MIN_FREQ, 42);
        let a = compose(&map, &r);
        let json = serde_json::to_string(&r).expect("a recipe serializes");
        let back: Recipe = serde_json::from_str(&json).expect("and parses");
        assert_eq!(back, r);
        let b = compose(&map, &back);
        assert_eq!(a.heights, b.heights);
        assert_eq!(a.sea, b.sea);
        let m = metrics(&map, &a);
        assert!(m.land_share > 0.05 && m.land_share < 0.95, "{m:?}");
        assert!(m.continents + m.islands >= 1, "{m:?}");
        assert!(m.ridges_on_edges >= 0.0 && m.ridges_on_edges <= 1.0);
        assert!(score(&m).is_finite());

        // The plateau flattens the interiors (more land near the crest value) and
        // the ridge adds height ONLY along the wave's contour band.
        let mut flat = r.clone();
        flat.harmonics.clear();
        flat.heat.emitters_on = false;
        flat.wave.mask.on = false;
        flat.coast.on = false; // isolate the wave and its own ridge
        let d = compose(&map, &flat);
        flat.wave.plateau = 0.5;
        let p = compose(&map, &flat);
        let near_crest = |c: &Composed| c.heights.iter().filter(|h| **h > 0.6 * 0.04).count();
        assert!(
            near_crest(&p) > near_crest(&d),
            "a plateau lifts the mid-slopes"
        );
        flat.wave.plateau = 1.0;
        flat.wave.ridge.on = true;
        let rr = compose(&map, &flat);
        let wave = PlanetWave::with_lobe(&map, flat.wave.seed, flat.wave.lobe);
        for t in 0..map.len() {
            let v = wave.value(t as TileId);
            let added = rr.heights[t] - d.heights[t];
            let on_band = (v - flat.wave.ridge.level).abs() < flat.wave.ridge.width;
            if on_band {
                assert!(
                    added > 0.3 * flat.wave.ridge.amp,
                    "tile {t}: the ridge stands on its band"
                );
            } else if (v - flat.wave.ridge.level).abs() > 3.0 * flat.wave.ridge.width {
                assert!(added < 1e-4, "tile {t}: and nowhere far from it");
            }
        }

        // The coast band raises ONLY the altitude belt: tiles far above or far
        // below the belt are untouched, tiles on it rise by the full amount.
        let mut unbanded = r.clone();
        unbanded.coast.on = false;
        let mut banded = unbanded.clone();
        banded.coast.on = true;
        let base = compose(&map, &unbanded);
        let cb = compose(&map, &banded);
        let (level, width, amp) = (banded.coast.level, banded.coast.width, banded.coast.amp);
        let (mut on_belt, mut off_belt) = (0, 0);
        for t in 0..map.len() {
            let rel = base.heights[t] - base.sea;
            let added = cb.heights[t] - base.heights[t];
            if (rel - level).abs() < 0.2 * width {
                assert!(added > 0.9 * amp, "tile {t}: on the belt the ridge stands");
                on_belt += 1;
            } else if (rel - level).abs() > 4.0 * width || rel <= 0.0 {
                assert!(
                    added < 1e-6,
                    "tile {t}: off the belt (and under the sea) nothing changes"
                );
                off_belt += 1;
            }
        }
        assert!(on_belt > 0 && off_belt > 0);

        let mut off = r.clone();
        off.wave.on = false;
        off.heat.on = false;
        for h in &mut off.harmonics {
            h.on = false;
        }
        let flat = compose(&map, &off);
        assert!(flat.heights.iter().all(|h| *h == 0.0));
        assert_eq!(
            metrics(&map, &flat).land_share,
            0.0,
            "a flat sphere at the datum is sea"
        );

        let (w, h) = (128u32, 64u32);
        let img = render_equirect(&map, &a, w, h, 0.01);
        assert_eq!(img.len(), (w * h * 3) as usize);
        let blue = img
            .chunks(3)
            .filter(|p| u16::from(p[2]) > u16::from(p[0]) + 40)
            .count();
        let green = img
            .chunks(3)
            .filter(|p| u16::from(p[1]) > u16::from(p[2]) + 20)
            .count();
        assert!(blue > 0 && green > 0, "sea {blue} land {green}");
    }
}
