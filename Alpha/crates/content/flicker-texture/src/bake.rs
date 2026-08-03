//! The baker — a recipe and a size in, a map set out.
//!
//! One pass evaluates the field into a height buffer; the maps are then read off
//! that buffer. Doing it in two stages is what makes relief and occlusion
//! possible at all: both are *neighbourhood* operations, and a texel cannot see
//! its neighbours while it is being generated.
//!
//! The neighbourhood always **wraps**. That is the second half of seamlessness —
//! a tiling field sampled with a clamped gradient still produces a visible ridge
//! at the border, because the edge texels' normals disagree with the ones they
//! meet when the texture repeats.

use crate::channel::mix;
use crate::recipe::TextureRecipe;

/// Which map a buffer is — the selector the bench's preview cycles, and the
/// filename suffix each is written under.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MapKind {
    /// Albedo. sRGB — the only map that is a colour.
    BaseColor,
    /// Tangent-space normals, `(128,128,255)` = flat.
    Normal,
    /// Linear scalar in R.
    Roughness,
    /// Linear scalar in R.
    Metallic,
    /// Linear scalar in R — ambient occlusion.
    Ao,
    /// Linear scalar in R — the raw field, for terrain displacement.
    Height,
}

impl MapKind {
    /// Every map a bake produces, in the order the bench's selector steps.
    pub const ALL: [MapKind; 6] = [
        MapKind::BaseColor,
        MapKind::Normal,
        MapKind::Roughness,
        MapKind::Metallic,
        MapKind::Ao,
        MapKind::Height,
    ];

    /// The content standard's map-role name — the `<Asset>_<Map>` filename
    /// suffix (`Alpha/content/README.md`). These are the names the rest of the
    /// tree already uses (`GolemBase_Low_BaseColor.png`), so a Sablework bake
    /// drops into the package tree looking like everything else.
    pub fn role(self) -> &'static str {
        match self {
            MapKind::BaseColor => "BaseColor",
            MapKind::Normal => "Normal",
            MapKind::Roughness => "Roughness",
            MapKind::Metallic => "Metallic",
            MapKind::Ao => "AO",
            MapKind::Height => "Height",
        }
    }

    /// Whether the map holds **colour** (sRGB-encoded) or **data** (linear).
    ///
    /// Load-bearing: a normal or roughness map uploaded through the sRGB path is
    /// silently wrong — the bytes get a gamma curve applied to numbers that were
    /// never a colour. The renderer has two entry points for exactly this reason
    /// (`load_texture` vs `load_texture_linear`), and this is what tells a caller
    /// which to use.
    pub fn is_color(self) -> bool {
        matches!(self, MapKind::BaseColor)
    }
}

/// A baked map: RGBA8, `size × size`.
///
/// Always RGBA even for the scalar maps, because that is what the renderer's
/// upload path takes; a scalar map replicates its value across RGB with an opaque
/// alpha, so it is also directly viewable in the 2D preview.
#[derive(Clone, Debug, PartialEq)]
pub struct Map {
    pub kind: MapKind,
    pub size: u32,
    /// `size * size * 4` bytes, row-major from the top-left.
    pub pixels: Vec<u8>,
}

/// Everything one recipe bakes to.
#[derive(Clone, Debug, PartialEq)]
pub struct MapSet {
    pub size: u32,
    pub maps: Vec<Map>,
}

impl MapSet {
    /// The map of a given kind. Always present for every [`MapKind::ALL`] after a
    /// [`bake`], so the bench can index without a fallback.
    pub fn get(&self, kind: MapKind) -> Option<&Map> {
        self.maps.iter().find(|m| m.kind == kind)
    }
}

use serde::{Deserialize, Serialize};

/// The occlusion ring's radius as a fraction of the tile — a cavity a twelfth of
/// the swatch across is the scale that reads as surface depth rather than as
/// grain. Fixed rather than authored: it is the definition of "a cavity" for this
/// baker, and one more slider here would only let an author break the map.
const AO_RING_DIVISOR: u32 = 12;

/// Converts the ring's height drop into darkening. Sized so a fully-authored
/// `ao` of 1 over a drop of a quarter of the field's range reaches full
/// occlusion — deep enough to read, short of crushing ordinary relief to black.
const AO_GAIN: f32 = 6.0;

/// Quantize a `[0,1]` scalar to a byte.
///
/// Rounds rather than truncates: truncation biases every map downward by half a
/// level and makes a flat 1.0 field quantize to 254.
fn q(v: f32) -> u8 {
    (v.clamp(0.0, 1.0) * 255.0).round() as u8
}

/// Read the height buffer with **wrapping** indices, so every neighbourhood
/// operation sees across the seam.
fn at(field: &[f32], size: u32, x: i64, y: i64) -> f32 {
    let n = size as i64;
    let (wx, wy) = (x.rem_euclid(n) as usize, y.rem_euclid(n) as usize);
    field[wy * size as usize + wx]
}

/// Evaluate the rack into a `size × size` field of `[0,1]` values.
///
/// Exposed because the bench previews the raw field while dragging and only needs
/// the full map set when it commits — and because it is the natural unit to test
/// tiling on.
pub fn field(recipe: &TextureRecipe, size: u32) -> Vec<f32> {
    let n = size.max(1);
    let inv = 1.0 / n as f64;
    let mut out = vec![0.0f32; (n as usize) * (n as usize)];
    for y in 0..n {
        for x in 0..n {
            // Texel CENTRES would be the usual choice, but the tile's seam must
            // land on a lattice line: sampling at `x/n` puts column 0 exactly on
            // the period boundary, which is what makes the wrap bit-exact.
            let u = x as f64 * inv;
            let v = y as f64 * inv;
            out[(y * n + x) as usize] = mix(&recipe.channels, u, v, recipe.seed) as f32;
        }
    }
    out
}

/// Bake a recipe into the full map set at `size × size`.
///
/// `size` is clamped to at least 1. Cost is `O(size² · enabled_channels)`; a 256²
/// preview is a fraction of a frame and a 2048² commit is a moment.
pub fn bake(recipe: &TextureRecipe, size: u32) -> MapSet {
    let n = size.max(1);
    let h = field(recipe, n);
    let count = (n as usize) * (n as usize);
    let out = &recipe.out;

    let mut base = Vec::with_capacity(count * 4);
    let mut normal = Vec::with_capacity(count * 4);
    let mut rough = Vec::with_capacity(count * 4);
    let mut metal = Vec::with_capacity(count * 4);
    let mut ao = Vec::with_capacity(count * 4);
    let mut height = Vec::with_capacity(count * 4);

    // The gradient's run: one texel, in tile units. Scaling relief by `n` would
    // make a 2048² bake eight times as bumpy as its 256² preview for the same
    // slider — the map must describe the SURFACE, not the sampling rate.
    let step = 2.0 / n as f32;

    // Occlusion is a CAVITY measure, so its ring must sit a fixed fraction of the
    // tile away rather than one texel away. Sampling the immediate neighbours
    // instead makes the map vanish as resolution rises — at 1024² two adjacent
    // texels of a smooth field differ by ~0.001, which rounds to no occlusion at
    // all — and that is the same resolution-dependence `step` exists to keep out
    // of the normal map.
    let ring_r = (n / AO_RING_DIVISOR).max(1) as i64;

    for y in 0..n as i64 {
        for x in 0..n as i64 {
            let v = at(&h, n, x, y);

            let c = out.ramp.sample(v);
            base.extend_from_slice(&[q(c[0]), q(c[1]), q(c[2]), 255]);

            // Central differences on the wrapped field → the surface slope, then
            // the tangent-space normal of that slope, encoded to `[0,255]`.
            let dx = (at(&h, n, x + 1, y) - at(&h, n, x - 1, y)) * out.relief / step;
            let dy = (at(&h, n, x, y + 1) - at(&h, n, x, y - 1)) * out.relief / step;
            let len = (dx * dx + dy * dy + 1.0).sqrt();
            let (nx, ny, nz) = (-dx / len, -dy / len, 1.0 / len);
            normal.extend_from_slice(&[
                q(nx * 0.5 + 0.5),
                q(ny * 0.5 + 0.5),
                q(nz * 0.5 + 0.5),
                255,
            ]);

            let r = q(out.roughness_at(v));
            rough.extend_from_slice(&[r, r, r, 255]);
            let m = q(out.metalness_at(v));
            metal.extend_from_slice(&[m, m, m, 255]);

            // Occlusion: how far this texel sits below the surface a cavity-radius
            // away, on all eight compass points. A texel level with or above its
            // surroundings is fully open.
            let r = ring_r;
            let ring = [
                at(&h, n, x - r, y),
                at(&h, n, x + r, y),
                at(&h, n, x, y - r),
                at(&h, n, x, y + r),
                at(&h, n, x - r, y - r),
                at(&h, n, x + r, y - r),
                at(&h, n, x - r, y + r),
                at(&h, n, x + r, y + r),
            ];
            let mean = ring.iter().sum::<f32>() / ring.len() as f32;
            let occ = (1.0 - (mean - v).max(0.0) * out.ao * AO_GAIN).clamp(0.0, 1.0);
            let a = q(occ);
            ao.extend_from_slice(&[a, a, a, 255]);

            let hv = q(v);
            height.extend_from_slice(&[hv, hv, hv, 255]);
        }
    }

    let map = |kind, pixels| Map { kind, size: n, pixels };
    MapSet {
        size: n,
        maps: vec![
            map(MapKind::BaseColor, base),
            map(MapKind::Normal, normal),
            map(MapKind::Roughness, rough),
            map(MapKind::Metallic, metal),
            map(MapKind::Ao, ao),
            map(MapKind::Height, height),
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::channel::{BlendMode, Channel, NoiseKind, CHANNEL_COUNT};

    /// A rack with one voice per source kind and a warp on — the busiest recipe
    /// the tests can build, so a seam or a NaN has nowhere to hide.
    fn busy_recipe() -> TextureRecipe {
        let mut r = TextureRecipe::default();
        for (i, kind) in NoiseKind::ALL.iter().take(CHANNEL_COUNT).enumerate() {
            r.channels[i] = Channel {
                enabled: true,
                source: *kind,
                scale: 3 + i as u32,
                warp: 0.3,
                salt: i as u64 * 17,
                blend: BlendMode::ALL[i % BlendMode::ALL.len()],
                amount: 0.6,
                ..Channel::default()
            };
        }
        r.out.metalness_mod = 0.4;
        r
    }

    /// The field is periodic on the tile — the mathematical half of seamlessness,
    /// asserted on the mixer directly so a failure points at the rack rather than
    /// at the baker.
    #[test]
    fn the_mixed_field_is_periodic_on_the_tile() {
        let r = busy_recipe();
        for (u, v) in [(0.0, 0.0), (0.3, 0.7), (0.9, 0.1), (0.5, 0.5)] {
            let base = mix(&r.channels, u, v, r.seed);
            for (du, dv) in [(1.0, 0.0), (0.0, 1.0), (1.0, 1.0), (-1.0, 2.0)] {
                let shifted = mix(&r.channels, u + du, v + dv, r.seed);
                assert!(
                    (base - shifted).abs() < 1e-9,
                    "field not periodic at ({u},{v}) + ({du},{dv}): {base} vs {shifted}"
                );
            }
        }
    }

    /// THE product guarantee: the baked BYTES wrap. The two edges of a tile are
    /// *adjacent* when it repeats, not identical — so the test is that the step
    /// across the seam is no bigger than an ordinary step between neighbouring
    /// rows. A seam is exactly "this join is worse than the others", and that is
    /// what a clamped (rather than wrapping) neighbourhood produces in `Normal`
    /// and `Ao`.
    #[test]
    fn no_baked_map_has_a_seam() {
        let size = 64u32;
        let set = bake(&busy_recipe(), size);
        for m in &set.maps {
            let px = |x: u32, y: u32| {
                let i = ((y * size + x) * 4) as usize;
                &m.pixels[i..i + 3]
            };
            let step = |a: &[u8], b: &[u8]| -> i32 {
                a.iter().zip(b).map(|(x, y)| (*x as i32 - *y as i32).abs()).max().unwrap_or(0)
            };

            // The worst ordinary neighbour step anywhere in the interior — the bar
            // the seam has to clear.
            let mut interior = 0;
            for y in 0..size {
                for x in 0..size - 1 {
                    interior = interior.max(step(px(x, y), px(x + 1, y)));
                    interior = interior.max(step(px(y, x), px(y, x + 1)));
                }
            }

            for k in 0..size {
                let x_seam = step(px(size - 1, k), px(0, k));
                assert!(
                    x_seam <= interior,
                    "{:?} x-seam at row {k} steps {x_seam}, worse than the interior's {interior}",
                    m.kind
                );
                let y_seam = step(px(k, size - 1), px(k, 0));
                assert!(
                    y_seam <= interior,
                    "{:?} y-seam at column {k} steps {y_seam}, worse than the interior's {interior}",
                    m.kind
                );
            }
        }
    }

    /// Determinism is the contract the whole crate rests on: the bench re-bakes
    /// constantly, and a recipe committed today must rebuild identically later.
    #[test]
    fn baking_is_deterministic() {
        let r = busy_recipe();
        assert_eq!(bake(&r, 32), bake(&r, 32));
    }

    /// Every map is fully populated and correctly sized — a short buffer would be
    /// a GPU upload crash, not a visual glitch.
    #[test]
    fn the_set_is_complete_and_correctly_sized() {
        let set = bake(&busy_recipe(), 16);
        assert_eq!(set.maps.len(), MapKind::ALL.len());
        for kind in MapKind::ALL {
            let m = set.get(kind).unwrap_or_else(|| panic!("{kind:?} missing"));
            assert_eq!(m.pixels.len(), 16 * 16 * 4, "{kind:?} wrong length");
            assert!(m.pixels.chunks(4).all(|p| p[3] == 255), "{kind:?} has non-opaque alpha");
        }
    }

    /// Only base colour is sRGB. Getting this backwards silently gamma-corrects
    /// data that is not a colour.
    #[test]
    fn exactly_one_map_is_a_colour() {
        let colour: Vec<_> = MapKind::ALL.iter().filter(|k| k.is_color()).collect();
        assert_eq!(colour, [&MapKind::BaseColor]);
    }

    /// Relief must describe the surface, not the sampling rate: the same recipe
    /// baked at two resolutions should agree about which way the surface tips.
    #[test]
    fn relief_is_resolution_independent() {
        let mut r = TextureRecipe::default();
        r.channels[0] = Channel { enabled: true, source: NoiseKind::Fbm, scale: 2, ..Channel::default() };
        r.out.relief = 1.0;

        let mean_tilt = |size: u32| {
            let m = bake(&r, size);
            let n = m.get(MapKind::Normal).unwrap();
            let sum: f64 = n.pixels.chunks(4).map(|p| (p[2] as f64 - 128.0).abs()).sum();
            sum / (n.pixels.len() / 4) as f64
        };
        let (a, b) = (mean_tilt(64), mean_tilt(256));
        assert!((a - b).abs() < a.max(b) * 0.25, "relief drifted with resolution: {a} vs {b}");
    }

    /// Occlusion must describe the surface, not the sampling rate — the same
    /// guarantee [`relief_is_resolution_independent`] makes for normals.
    ///
    /// This is the test whose absence let AO ship blank: measured against the
    /// ±1-texel neighbours it faded to nothing as resolution rose, so it looked
    /// plausible at 64² and was pure white at 1024².
    #[test]
    fn occlusion_is_resolution_independent_and_actually_darkens() {
        let mut r = TextureRecipe::default();
        r.channels[0] =
            Channel { enabled: true, source: NoiseKind::Fbm, scale: 4, ..Channel::default() };
        r.out.ao = 1.0;

        let mean_occlusion = |size: u32| {
            let set = bake(&r, size);
            let m = set.get(MapKind::Ao).unwrap();
            let sum: f64 = m.pixels.chunks(4).map(|p| p[0] as f64).sum();
            sum / (m.pixels.len() / 4) as f64
        };
        let (small, large) = (mean_occlusion(64), mean_occlusion(512));
        assert!(small < 250.0, "AO is blank at 64²: mean {small}");
        assert!(large < 250.0, "AO is blank at 512²: mean {large}");
        assert!(
            (small - large).abs() < 16.0,
            "AO drifted with resolution: {small} at 64² vs {large} at 512²"
        );
    }

    /// A flat field must produce a flat normal map — the sanity check that the
    /// gradient encoding is centred where the shader expects it.
    #[test]
    fn a_flat_field_bakes_a_flat_normal() {
        // No enabled channels ⇒ the bus stays at zero ⇒ a perfectly flat field.
        let r = TextureRecipe {
            channels: [Channel::default(); CHANNEL_COUNT],
            ..TextureRecipe::default()
        };
        let set = bake(&r, 8);
        let n = set.get(MapKind::Normal).unwrap();
        for p in n.pixels.chunks(4) {
            assert_eq!((p[0], p[1], p[2]), (128, 128, 255), "not flat: {p:?}");
        }
    }

    /// A degenerate size must not panic — the preview resizes freely.
    #[test]
    fn a_zero_size_bakes_one_texel() {
        let set = bake(&TextureRecipe::default(), 0);
        assert_eq!(set.size, 1);
        assert!(set.maps.iter().all(|m| m.pixels.len() == 4));
    }
}
