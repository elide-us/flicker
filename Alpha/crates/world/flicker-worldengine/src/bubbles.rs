//! **BUBBLES ON BUBBLES** — the rounded field kind (Aaron 2026-10-15,
//! 7F7A53DA: *"everything is bubbles … layers of bubbles, bubbles with
//! bubbles on them, bubbles under bubbles. Everything is a rounded
//! surface"*). Beside the resonance trains — sinusoids over whole-planet axes
//! — this is the other vocabulary of the shape: DOMES and BOWLS with a
//! footprint, a smooth rounded cap, positive or negative, exactly zero
//! outside its own radius, at a SPECTRUM of sizes — every octave's caps half
//! the radius of the last — stacked by superposition: big bubbles carrying
//! medium bubbles carrying small ones, bowls under domes. Where erosion has
//! not touched the land, this is what the ground is.
//!
//! **A field of position.** An octave is an equal-area BAND GRID over the
//! sphere at the octave's spacing; every cell rolls its own bubble from the
//! recipe's seed and its indices alone — whether it stands at all, where in
//! the cell it sits, dome or bowl, how wide, how high — so the value at any
//! direction is the sum of the few caps in reach, found by walking the bands
//! around the point. A coarse octave's caps are tabled once (a cache of the
//! roll, never a source of it); a fine octave — millions of cells — rolls
//! each cap as it is asked for, from the same one definition. So the same
//! `(recipe, p)` is the same height at every resolution — the hex grid
//! ([`crate::recipe::compose`]) carries the octaves it resolves, the pixel
//! tier evaluates every octave exactly at every pixel, and two hexes agree
//! at their seam by construction. Not noise: every cap is a closed form with
//! a footprint you can walk around.

use glam::{DVec3, Vec3};
use serde::{Deserialize, Serialize};
use std::f32::consts::{FRAC_PI_2, PI, TAU};

use crate::map::RESOLVE_TILES;

/// **The bubble dials.**
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct BubbleRecipe {
    /// The roll every cap derives from.
    pub seed: u64,
    /// Octaves from the largest down, each half the radius of the last.
    pub octaves: u32,
    /// The largest octave's cap radius, radians of arc.
    pub largest: f32,
    /// The largest octave's cap height, radius units.
    pub amp: f32,
    /// The spectral tilt: height falls `2^(−tilt)` an octave (1 keeps every
    /// octave's slope; under 1 the small bubbles stand relatively steeper).
    pub tilt: f32,
    pub on: bool,
}

impl BubbleRecipe {
    /// From a continent-sized swell (0.35 rad, ~2 200 km across at Earth's
    /// radius) down thirteen octaves to ~550 m caps — fourteen pixels of
    /// the 2K hex: the hex grid carries the first few, the pixel tier the
    /// rest. The tilt of a half: height falls as the square root of the
    /// radius, so the small bubbles stand relatively steeper — walked ground
    /// is rougher for its size than a continent is.
    pub const DEFAULT_OCTAVES: u32 = 13;
    pub const DEFAULT_LARGEST: f32 = 0.35;
    pub const DEFAULT_AMP: f32 = 0.004;
    pub const DEFAULT_TILT: f32 = 0.42;

    /// The bubbles rolled off `seed` at the opening dials.
    pub fn rolled(seed: u64) -> Self {
        Self {
            seed,
            octaves: Self::DEFAULT_OCTAVES,
            largest: Self::DEFAULT_LARGEST,
            amp: Self::DEFAULT_AMP,
            tilt: Self::DEFAULT_TILT,
            on: true,
        }
    }

    /// Octave `k`'s cap radius, radians.
    pub fn radius(&self, k: u32) -> f32 {
        self.largest / 2f32.powi(k as i32)
    }

    /// Octave `k`'s cap height, radius units.
    pub fn height(&self, k: u32) -> f32 {
        self.amp * 2f32.powf(-(k as f32) * self.tilt)
    }
}

impl Default for BubbleRecipe {
    fn default() -> Self {
        Self::rolled(0)
    }
}

/// Cell spacing in cap radii: caps a little apart on average, overlapping
/// where the jitter brings them together.
pub const BUBBLE_SPACING: f32 = 1.6;
/// How far a cap's centre strays from its cell's centre, in cell spans —
/// enough that the band grid never reads as rows.
pub const BUBBLE_JITTER: f32 = 0.8;
/// The share of cells that hold a bubble — footprints with ground between.
pub const BUBBLE_FILL: f32 = 0.7;
/// Domes among the bubbles that stand; the rest are bowls.
pub const BUBBLE_DOME_SHARE: f32 = 0.5;
/// A cap's own radius and height stray down from the octave's by up to these shares.
pub const BUBBLE_RADIUS_STRAY: f32 = 0.4;
pub const BUBBLE_HEIGHT_STRAY: f32 = 0.5;

/// One cap of an octave's grid: its centre, the SQUARED CHORD of its radius
/// (zero for an empty cell — nothing is ever that close), and its signed
/// height.
///
/// The footprint is read by chord, not by the cosine of the angle: a 500 m
/// cap is 85 µrad across, whose cosine is 1 − 4·10⁻⁹ — exactly 1.0 in f32 —
/// so an angle test fires only on rounding noise and plants full-height
/// spikes at random pixels (the first fine octaves read as sandpaper,
/// 2026-10-16). The chord between two unit vectors keeps its precision down
/// to the metre, and costs no arc-cosine.
#[derive(Clone, Copy)]
struct Cap {
    centre: Vec3,
    chord2: f32,
    height: f32,
}

impl Cap {
    /// The cap's radius, radians of arc, back from its chord.
    #[cfg(test)]
    fn radius(&self) -> f32 {
        2.0 * (self.chord2.sqrt() * 0.5).asin()
    }
}

const EMPTY: Cap = Cap {
    centre: Vec3::Y,
    chord2: 0.0,
    height: 0.0,
};

/// One band of an octave's equal-area grid: its cell count, its longitude
/// offset (in cells), its latitude's cosine and sine, where its cells start
/// in a table, and the cells either side a cap can reach.
struct Band {
    cells: u32,
    offset: f32,
    cos_lat: f32,
    sin_lat: f32,
    start: u32,
    reach: u32,
}

/// One octave: its equal-area band grid, and its caps — TABLED when the grid
/// is small enough (a cache of the roll, never a source of it), rolled on
/// the fly otherwise (the fine octaves run to millions of cells).
struct Octave {
    k: u32,
    seed: u64,
    radius: f32,
    height: f32,
    band_h: f32,
    /// Bands a cap can reach past the one a point lies in.
    band_reach: u32,
    bands: Vec<Band>,
    caps: Option<Vec<Cap>>,
}

/// An octave with more cells than this rolls its caps on the fly.
pub const TABLE_MAX_CELLS: usize = 1_500_000;

/// splitmix64's finaliser: the roll's whole state from a few indices.
fn mix(mut z: u64) -> u64 {
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// The roll of cell `(k, j, i)` — a pure function of the seed and the indices.
fn roll(seed: u64, k: u32, j: u32, i: u32) -> u64 {
    mix(seed
        ^ mix((u64::from(k) << 56) | (u64::from(j) << 28) | u64::from(i)).wrapping_add(0x5BD1_E995))
}

/// Sixteen bits of a roll as a unit float, `n` in `0..4`.
fn unit(h: u64, n: u32) -> f32 {
    ((h >> (16 * n)) & 0xFFFF) as f32 / 65536.0
}

impl Octave {
    fn build(r: &BubbleRecipe, k: u32) -> Self {
        let radius = r.radius(k);
        let height = r.height(k);
        let spacing = radius * BUBBLE_SPACING;
        let count = ((PI / spacing).ceil() as u32).max(1);
        let band_h = PI / count as f32;
        let band_reach = (radius / band_h + BUBBLE_JITTER * 0.5).ceil() as u32;
        let mut bands = Vec::with_capacity(count as usize);
        let mut total = 0u32;
        for j in 0..count {
            let lat = -FRAC_PI_2 + (j as f32 + 0.5) * band_h;
            let cells = ((TAU * lat.cos() / band_h).round() as u32).max(1);
            let ground_w = TAU / cells as f32 * lat.cos().max(1e-3);
            bands.push(Band {
                cells,
                offset: unit(roll(r.seed, k, j, u32::MAX), 0),
                cos_lat: lat.cos(),
                sin_lat: lat.sin(),
                start: total,
                reach: ((radius + BUBBLE_JITTER * 0.5 * ground_w) / ground_w).ceil() as u32,
            });
            total += cells;
        }
        let mut octave = Self {
            k,
            seed: r.seed,
            radius,
            height,
            band_h,
            band_reach,
            bands,
            caps: None,
        };
        if total as usize <= TABLE_MAX_CELLS {
            let mut caps = Vec::with_capacity(total as usize);
            for j in 0..count {
                for i in 0..octave.bands[j as usize].cells {
                    caps.push(octave.cap_of(j, i));
                }
            }
            octave.caps = Some(caps);
        }
        octave
    }

    /// **The cap of cell `(j, i)`** — the one definition, tabled or not: from
    /// the roll alone, whether it stands, where in the cell (a stray of
    /// [`BUBBLE_JITTER`] in both directions), dome or bowl, its radius and
    /// height under the octave's.
    fn cap_of(&self, j: u32, i: u32) -> Cap {
        let h1 = roll(self.seed, self.k, j, i);
        if unit(h1, 0) >= BUBBLE_FILL {
            return EMPTY;
        }
        let h2 = mix(h1 ^ 0xA5A5_5A5A_F00D_BEEF);
        let band = &self.bands[j as usize];
        let cell_w = TAU / band.cells as f32;
        // The latitude stray is small: its cosine and sine by series, the
        // band's own by table — no trigonometry per cell but the longitude's.
        let d = (unit(h1, 1) - 0.5) * BUBBLE_JITTER * self.band_h;
        let d2 = d * d;
        let (cos_d, sin_d) = (
            1.0 - d2 * (0.5 - d2 / 24.0),
            d * (1.0 - d2 * (1.0 / 6.0 - d2 / 120.0)),
        );
        let cos_lat = band.cos_lat * cos_d - band.sin_lat * sin_d;
        let sin_lat = band.sin_lat * cos_d + band.cos_lat * sin_d;
        let lon = (i as f32 + 0.5 + band.offset + (unit(h1, 2) - 0.5) * BUBBLE_JITTER) * cell_w;
        let rr = self.radius * (1.0 - BUBBLE_RADIUS_STRAY * unit(h1, 3));
        let sign = if unit(h2, 0) < BUBBLE_DOME_SHARE {
            1.0
        } else {
            -1.0
        };
        let chord = 2.0 * (rr * 0.5).sin();
        Cap {
            centre: Vec3::new(cos_lat * lon.cos(), sin_lat, cos_lat * lon.sin()),
            chord2: chord * chord,
            height: self.height * (1.0 - BUBBLE_HEIGHT_STRAY * unit(h2, 1)) * sign,
        }
    }

    /// [`Self::eval`] in double precision: the same caps, the chord in f64
    /// (a cap's own position is its f32 roll, carried exactly).
    fn eval64(&self, p: DVec3, lat: f64, lon: f64) -> f64 {
        let count = self.bands.len() as i64;
        let band_h = f64::from(self.band_h);
        let j0 =
            (((lat + std::f64::consts::FRAC_PI_2) / band_h).floor() as i64).clamp(0, count - 1);
        let lon = lon.rem_euclid(std::f64::consts::TAU);
        let mut sum = 0.0f64;
        let br = i64::from(self.band_reach);
        for j in (j0 - br)..=(j0 + br) {
            if j < 0 || j >= count {
                continue;
            }
            let band = &self.bands[j as usize];
            let n = i64::from(band.cells);
            let cell_w = std::f64::consts::TAU / n as f64;
            let i0 = (lon / cell_w - f64::from(band.offset)).floor() as i64;
            let reach = i64::from(band.reach);
            let (lo, hi) = if 2 * reach + 1 >= n {
                (0, n - 1)
            } else {
                (i0 - reach, i0 + reach)
            };
            for i in lo..=hi {
                let i = i.rem_euclid(n) as u32;
                let cap = match &self.caps {
                    Some(caps) => caps[(band.start + i) as usize],
                    None => self.cap_of(j as u32, i),
                };
                sum += cap_at64(&cap, p);
            }
        }
        sum
    }

    /// The octave's height at `p` (unit), whose latitude and longitude are
    /// `lat` and `lon`: the caps in reach, summed.
    fn eval(&self, p: Vec3, lat: f32, lon: f32) -> f32 {
        let count = self.bands.len() as i64;
        let j0 = (((lat + FRAC_PI_2) / self.band_h).floor() as i64).clamp(0, count - 1);
        let lon = lon.rem_euclid(TAU);
        let mut sum = 0.0f32;
        let br = i64::from(self.band_reach);
        for j in (j0 - br)..=(j0 + br) {
            if j < 0 || j >= count {
                continue;
            }
            let band = &self.bands[j as usize];
            let n = i64::from(band.cells);
            let cell_w = TAU / n as f32;
            let i0 = (lon / cell_w - band.offset).floor() as i64;
            let reach = i64::from(band.reach);
            let (lo, hi) = if 2 * reach + 1 >= n {
                (0, n - 1)
            } else {
                (i0 - reach, i0 + reach)
            };
            for i in lo..=hi {
                let i = i.rem_euclid(n) as u32;
                let cap = match &self.caps {
                    Some(caps) => caps[(band.start + i) as usize],
                    None => self.cap_of(j as u32, i),
                };
                sum += cap_at(&cap, p);
            }
        }
        sum
    }
}

/// **The cap** — a rounded dome or bowl with a footprint: its full height at
/// the centre, falling as `(1 − (c/C)²)²` — `c` the chord from the centre,
/// `C` the chord of its radius — to exactly zero at its radius with zero
/// slope there (nothing is a plane, nothing has a rim), zero beyond.
fn cap_at(cap: &Cap, p: Vec3) -> f32 {
    let c2 = (cap.centre - p).length_squared();
    if c2 >= cap.chord2 {
        return 0.0;
    }
    let s = 1.0 - c2 / cap.chord2;
    cap.height * s * s
}

/// [`cap_at`] in double precision.
fn cap_at64(cap: &Cap, p: DVec3) -> f64 {
    let c2 = (cap.centre.as_dvec3() - p).length_squared();
    let chord2 = f64::from(cap.chord2);
    if c2 >= chord2 {
        return 0.0;
    }
    let s = 1.0 - c2 / chord2;
    f64::from(cap.height) * s * s
}

/// **The bubble field**: the recipe's octaves `from..to`, tabled.
pub struct Bubbles {
    from: u32,
    to: u32,
    octaves: Vec<Octave>,
}

impl Bubbles {
    /// Every octave of the recipe — the pixel tier's field.
    pub fn new(r: &BubbleRecipe) -> Self {
        Self::octaves_of(r, 0, r.octaves)
    }

    /// Only the octaves the hex grid resolves — caps at least
    /// [`RESOLVE_TILES`] tile angles across, the grid's one rule for every
    /// field — the composition's.
    pub fn resolved_by(r: &BubbleRecipe, tile_angle: f32) -> Self {
        let to = (0..r.octaves)
            .take_while(|k| r.radius(*k) * 2.0 >= RESOLVE_TILES * tile_angle)
            .count() as u32;
        Self::octaves_of(r, 0, to)
    }

    fn octaves_of(r: &BubbleRecipe, from: u32, to: u32) -> Self {
        Self {
            from,
            to,
            octaves: (from..to).map(|k| Octave::build(r, k)).collect(),
        }
    }

    /// The octaves this field stands.
    pub fn octaves(&self) -> std::ops::Range<u32> {
        self.from..self.to
    }

    /// [`Self::eval`] in double precision — the ladder under the pixel.
    pub fn eval64(&self, p: DVec3) -> f64 {
        let p = p.normalize();
        let lat = p.y.clamp(-1.0, 1.0).asin();
        let lon = p.z.atan2(p.x);
        self.octaves.iter().map(|o| o.eval64(p, lat, lon)).sum()
    }

    /// The field at `p` (any direction), radius units.
    pub fn eval(&self, p: Vec3) -> f32 {
        let p = p.normalize();
        let lat = p.y.clamp(-1.0, 1.0).asin();
        let lon = p.z.atan2(p.x);
        self.octaves.iter().map(|o| o.eval(p, lat, lon)).sum()
    }

    /// One octave of the field at `p` (zero for an octave not stood).
    pub fn octave(&self, k: u32, p: Vec3) -> f32 {
        if k < self.from || k >= self.to {
            return 0.0;
        }
        let p = p.normalize();
        let lat = p.y.clamp(-1.0, 1.0).asin();
        let lon = p.z.atan2(p.x);
        self.octaves[(k - self.from) as usize].eval(p, lat, lon)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::map::{HexMap, MIN_FREQ};
    use crate::recipe::{compose, Recipe};
    use crate::rocks::Rock;

    fn directions(n: usize, seed: u64) -> Vec<Vec3> {
        let mut rng = fastrand::Rng::with_seed(seed);
        (0..n)
            .map(|_| {
                let z = rng.f32() * 2.0 - 1.0;
                let a = rng.f32() * TAU;
                let r = (1.0 - z * z).max(0.0).sqrt();
                Vec3::new(r * a.cos(), z, r * a.sin())
            })
            .collect()
    }

    /// **Bubbles are domes and bowls with footprints at every scale.** Every
    /// octave is pure and finite, within its height, and has ground between
    /// its caps (exact zeros), domes and bowls both; the octaves halve in
    /// radius and fall in height; a cap is rounded — from a dome's centre the
    /// height falls monotonically to exactly zero at its radius; the hex grid
    /// stands only the octaves it resolves; composed alone, the bubbles are
    /// the whole relief and carry their own hardness, and off, nothing.
    #[test]
    fn bubbles_are_domes_and_bowls_with_footprints_at_every_scale() {
        let r = BubbleRecipe::rolled(7);
        let b = Bubbles::new(&r);
        let dirs = directions(3000, 1);
        for k in 0..r.octaves {
            let h = r.height(k);
            let (mut zeros, mut domes, mut bowls) = (0, 0, 0);
            for &p in &dirs {
                let v = b.octave(k, p);
                // Caps overlap where the jitter brings them together: a few
                // heights at most, never a spike.
                assert!(
                    v.is_finite() && v.abs() <= 3.0 * h + 1e-6,
                    "octave {k}: {v} within three of {h}"
                );
                assert_eq!(v, b.octave(k, p), "pure");
                match v.partial_cmp(&0.0) {
                    Some(std::cmp::Ordering::Equal) => zeros += 1,
                    Some(std::cmp::Ordering::Greater) => domes += 1,
                    _ => bowls += 1,
                }
            }
            assert!(zeros > 0, "octave {k}: ground between the bubbles");
            assert!(
                domes > 0 && bowls > 0,
                "octave {k}: domes {domes} and bowls {bowls}"
            );
            if k > 0 {
                assert!((r.radius(k) * 2.0 - r.radius(k - 1)).abs() < 1e-6);
                assert!(r.height(k) < r.height(k - 1));
            }
        }
        // The whole field is the octaves' sum, and its double-precision
        // reading is the same field.
        for &p in dirs.iter().take(50) {
            let sum: f32 = (0..r.octaves).map(|k| b.octave(k, p)).sum();
            assert!((b.eval(p) - sum).abs() < 1e-6);
            assert!((b.eval64(p.as_dvec3()) - f64::from(b.eval(p))).abs() < 1e-5);
        }
        // A cap is rounded: from a dome's centre the height falls and never
        // rises, reaches exactly zero at its radius with no step (the last
        // inside reading is a sliver of the height), and stays zero beyond.
        let coarse = Bubbles::octaves_of(&r, 0, 1);
        let dome = *coarse.octaves[0]
            .caps
            .as_ref()
            .expect("the coarse octave is tabled")
            .iter()
            .find(|c| c.height > 0.0)
            .expect("a dome in the coarse octave");
        let dome = &dome;
        let side = dome.centre.any_orthonormal_vector();
        let mut last = cap_at(dome, dome.centre);
        assert!(
            (last - dome.height).abs() < 1e-7,
            "full height at the centre"
        );
        let mut inside_last = last;
        for step in 1..=400 {
            let theta = step as f32 * dome.radius() * 1.5 / 400.0;
            let p = (dome.centre * theta.cos() + side * theta.sin()).normalize();
            let v = cap_at(dome, p);
            assert!(v <= last + 1e-7, "rounded: {v} after {last}");
            if theta < dome.radius() - 1e-4 {
                assert!(v > 0.0, "inside the footprint");
                inside_last = v;
            } else if theta > dome.radius() + 1e-4 {
                assert_eq!(v, 0.0, "zero past the radius");
            }
            last = v;
        }
        assert!(inside_last < dome.height * 0.01, "no step at the edge");
        // The fine octaves roll on the fly from the same definition as a
        // table: an octave tabled by hand reads the same at every sample.
        let fine_k = r.octaves - 1;
        let fine = Bubbles::octaves_of(&r, fine_k, r.octaves);
        assert!(
            fine.octaves[0].caps.is_none(),
            "the finest octave is not tabled"
        );
        let mut tabled = Octave::build(&r, fine_k);
        let mut caps = Vec::new();
        for (j, band) in tabled.bands.iter().enumerate() {
            for i in 0..band.cells {
                caps.push(tabled.cap_of(j as u32, i));
            }
        }
        tabled.caps = Some(caps);
        for &p in dirs.iter().take(300) {
            let q = p.normalize();
            let (lat, lon) = (q.y.clamp(-1.0, 1.0).asin(), q.z.atan2(q.x));
            assert_eq!(fine.octaves[0].eval(q, lat, lon), tabled.eval(q, lat, lon));
        }
        // The hex grid stands only what it resolves.
        let map = HexMap::new(MIN_FREQ);
        let ta = map.tile_angle();
        let stood = Bubbles::resolved_by(&r, ta);
        assert!(stood.octaves().end > 0 && stood.octaves().end < r.octaves);
        assert!(stood
            .octaves()
            .all(|k| r.radius(k) * 2.0 >= RESOLVE_TILES * ta));
        assert_eq!(stood.octave(r.octaves - 1, dirs[0]), 0.0);
        // Composed alone: the bubbles are the relief and carry their hardness.
        let mut only = Recipe::default_at(MIN_FREQ, 7);
        only.wave.on = false;
        only.stack.on = false;
        only.heat.emitters_on = false;
        let c = compose(&map, &only);
        assert!(c.heights.iter().any(|h| *h != 0.0));
        for (h, hard) in c.heights.iter().zip(&c.hardness) {
            if h.abs() > 1e-9 {
                assert!(
                    (hard - Rock::Granite.resistance()).abs() < 1e-6,
                    "a bubble is a dome of the basement: {hard}"
                );
            }
        }
        let mut off = only;
        off.bubbles.on = false;
        assert!(compose(&map, &off).heights.iter().all(|h| *h == 0.0));
    }
}
